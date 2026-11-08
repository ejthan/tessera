// SPDX-License-Identifier: GPL-3.0-or-later

//! The raw console of a connection: any request, its answer, and a history of both.

mod list;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::base::actions::{SelectDown, SelectUp};
use gpui_kit::component::IndexPath;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tab::TabBar;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, h_flex, h_resizable, resizable_panel, v_flex,
};
use gpui_kit::{
    Action, App, AppContext as _, ClipboardItem, Context, Entity, FocusHandle, Focusable,
    IntoElement, ParentElement as _, PathPromptOptions, Render, SharedString, Styled as _,
    Subscription, Task, WeakEntity, Window, div, prelude::*, px,
};
use rust_i18n::t;
use tessera_client::{Client, Error, FULL_BODY_MAX, Method, READ_METHODS, RawBody, RawRequest};
use tessera_core::openapi::path_candidates;
use tessera_core::raw::{
    OutcomeNote, PathError, RAW_FILE_MAX, RawDanger, cap_body, outcome_note, parse_path,
    query_rows, raw_danger, raw_query,
};
use tessera_core::search::history::{HistoryEntry, HistoryItem, raw_entry};
use tessera_core::{curl, redact};
use tessera_widgets::confirm_danger::DangerOptions;
use tessera_widgets::{
    BodyView, CompletionInput, CompletionInputEvent, JsonEditor, NextChunk, PreviousChunk, confirm,
    confirm_danger,
};

use self::list::RequestList;
use super::dock::view_tab;
use super::env_badge;
use super::request_detail::size;
use crate::actions::{
    AddQueryRow, AttachFile, CONSOLE, CopyCurl, FavoriteRequest, LoadQuery, OpenHistory,
    RemoveBodyFile, RevealSecrets, SendRequest, ShowResponseBody, ShowResponseHeaders, StopWaiting,
};
use crate::connection::{ConnectionState, long_error_text};
use crate::runtime::Tokio;
use crate::search::SearchBackendGlobal;
use crate::state::AppState;

const METHODS: [&str; 6] = ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE"];
/// Index of the Text entry of the body kind.
const KIND_TEXT: usize = 1;
/// Width of the request's side of the split.
const REQUEST_WIDTH: f32 = 440.;

/// One row of the query table. A row with neither key nor value does not count.
struct QueryRow {
    key: Entity<InputState>,
    value: Entity<InputState>,
    _subscriptions: [Subscription; 2],
}

/// A body read from a file, kept outside the editor.
struct FileBody {
    name: String,
    text: Arc<str>,
    /// It holds a mask, found off the UI thread.
    masked: bool,
}

/// The answer on screen.
struct Answer {
    status: u16,
    duration: Duration,
    headers: Vec<(String, String)>,
    note: Option<OutcomeNote>,
    /// Bytes the server sent.
    size: usize,
    /// The text was cut at `FULL_BODY_MAX`.
    cut: bool,
    /// As the server sent it, up to `FULL_BODY_MAX`.
    text: Arc<str>,
    /// With secrets masked.
    masked: Arc<str>,
}

/// The answer's text worked out off the UI thread.
struct Worked {
    answer: tessera_client::RawResponse,
    note: Option<OutcomeNote>,
    /// The length before the cut, if the body was cut.
    cut_from: Option<usize>,
    masked: String,
}

fn path_error_text(error: &PathError) -> String {
    match error {
        PathError::Empty => t!("console.path.empty").to_string(),
        PathError::Character(c) => t!("console.path.character", char = c).to_string(),
        PathError::Segment(s) => t!("console.path.segment", segment = s).to_string(),
        PathError::BadEscape => t!("console.path.bad_escape").to_string(),
        PathError::Scheme => t!("console.path.scheme").to_string(),
    }
}

fn now() -> i64 {
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH);
    elapsed.map_or(0, |d| d.as_secs() as i64)
}

/// A connection's raw console.
pub struct ConsoleView {
    state: Entity<AppState>,
    connection: Entity<ConnectionState>,
    method: Entity<SelectState<Vec<String>>>,
    path: Entity<CompletionInput>,
    path_text: String,
    /// The collection list the candidates were made from.
    list_loaded: Option<std::time::Instant>,
    rows: Vec<QueryRow>,
    kind: Entity<SelectState<Vec<String>>>,
    body: Entity<JsonEditor>,
    file: Option<FileBody>,
    /// A note about what was loaded.
    note: Option<SharedString>,
    error: Option<SharedString>,
    answer: Option<Answer>,
    response: Entity<BodyView>,
    /// The answer shows unmasked, until the next Send.
    revealed: bool,
    on_headers: bool,
    /// Identifies the newest request; an answer of an older one is dropped.
    generation: u64,
    /// The running read. Replacing it drops the older one, which cancels its request.
    read_task: Option<Task<Result<(), SharedString>>>,
    /// A read waits for its answer.
    reading: bool,
    /// A write waits for its answer. Send stays busy.
    writing: bool,
    file_task: Option<Task<()>>,
    /// The check of the body before a send; a newer send replaces it.
    prepare_task: Option<Task<()>>,
    list: RequestList,
    focus_handle: FocusHandle,
    _subscriptions: [Subscription; 4],
}

impl ConsoleView {
    /// The console of `connection`, on a `GET`.
    pub fn new(
        state: Entity<AppState>,
        connection: Entity<ConnectionState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let methods: Vec<String> = METHODS.iter().map(ToString::to_string).collect();
        let first = Some(IndexPath::default());
        let method = cx.new(|cx| SelectState::new(methods, first, window, cx));
        let kinds = vec![
            t!("console.kind.json").to_string(),
            t!("console.kind.text").to_string(),
        ];
        let kind = cx.new(|cx| SelectState::new(kinds, first, window, cx));
        let path =
            cx.new(|cx| CompletionInput::new(t!("console.path.label").to_string(), window, cx));
        let body = cx.new(|cx| JsonEditor::new("", false, window, cx));
        let response = cx.new(|cx| BodyView::new(window, cx));
        if connection.read(cx).collection_cache.collections.is_none() {
            connection.update(cx, |c, cx| c.load_collection_list(cx));
        }
        let (list, list_changed) = RequestList::new(window, cx);
        let mut this = Self {
            _subscriptions: [
                list_changed,
                cx.subscribe_in(&path, window, |this, _, event, window, cx| match event {
                    CompletionInputEvent::Changed(text) => {
                        this.path_text.clone_from(text);
                        this.fill_candidates(cx);
                        cx.notify();
                    }
                    CompletionInputEvent::Submitted(_) => this.send(window, cx),
                }),
                cx.subscribe(&method, |_, _, _: &SelectEvent<Vec<String>>, cx| {
                    cx.notify()
                }),
                cx.observe(&connection, |this, _, cx| {
                    // The connection also changes with every poll.
                    let loaded = this.connection.read(cx).collection_cache.loaded_at;
                    if loaded != this.list_loaded {
                        this.list_loaded = loaded;
                        this.fill_candidates(cx);
                    }
                }),
            ],
            state,
            connection,
            method,
            path,
            path_text: String::new(),
            list_loaded: None,
            rows: Vec::new(),
            kind,
            body,
            file: None,
            note: None,
            error: None,
            answer: None,
            response,
            revealed: false,
            on_headers: false,
            generation: 0,
            read_task: None,
            reading: false,
            writing: false,
            file_task: None,
            prepare_task: None,
            list,
            focus_handle: cx.focus_handle(),
        };
        this.fill_candidates(cx);
        this
    }

    fn fill_candidates(&mut self, cx: &mut Context<Self>) {
        let cache = &self.connection.read(cx).collection_cache;
        let names: Vec<String> = cache
            .collections
            .iter()
            .flatten()
            .map(|c| c.name.clone())
            .collect();
        let candidates = path_candidates(&self.path_text, &names);
        self.path
            .update(cx, |input, _| input.set_candidates(candidates));
    }

    /// The method picked.
    pub fn method(&self, cx: &App) -> Method {
        let name = self.method.read(cx).selected_value();
        name.and_then(|m| Method::from_bytes(m.as_bytes()).ok())
            .unwrap_or(Method::GET)
    }

    /// Picks `method`, one of the six the console offers.
    pub fn set_method(&mut self, method: &str, window: &mut Window, cx: &mut Context<Self>) {
        let method = method.to_string();
        self.method
            .update(cx, |s, cx| s.set_selected_value(&method, window, cx));
        cx.notify();
    }

    /// The path as typed.
    pub fn path_text(&self) -> &str {
        &self.path_text
    }

    /// The path input.
    pub fn path_input(&self) -> &Entity<CompletionInput> {
        &self.path
    }

    /// Replaces the path.
    pub fn set_path(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.path
            .update(cx, |input, cx| input.set_value(path, window, cx));
    }

    /// Replaces the body in the editor.
    pub fn set_body(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.body.update(cx, |b, cx| b.set_text(text, window, cx));
    }

    /// Bytes of the answer the console holds.
    pub fn answer_len(&self) -> Option<usize> {
        Some(self.answer.as_ref()?.text.len())
    }

    /// The view of the answer.
    pub fn response_view(&self) -> &Entity<BodyView> {
        &self.response
    }

    fn has_body(&self, cx: &App) -> bool {
        !READ_METHODS.contains(&self.method(cx))
    }

    fn profile_id(&self, cx: &App) -> uuid::Uuid {
        self.connection.read(cx).profile.id
    }

    fn client(&self, cx: &App) -> Option<Client> {
        let connected = self.connection.read(cx).connected.as_ref()?;
        Some(connected.client.clone())
    }

    fn push_row(&mut self, key: &str, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        let input = |text: &str, label: String, window: &mut Window, cx: &mut Context<Self>| {
            let text = text.to_string();
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(label)
                    .default_value(text)
            })
        };
        let key = input(key, t!("console.query.key").to_string(), window, cx);
        let value = input(value, t!("console.query.value").to_string(), window, cx);
        let watch = |this: &mut Self, _, _: &InputEvent, cx: &mut Context<Self>| {
            this.error = None;
            cx.notify();
        };
        let _subscriptions = [cx.subscribe(&key, watch), cx.subscribe(&value, watch)];
        self.rows.push(QueryRow {
            key,
            value,
            _subscriptions,
        });
        cx.notify();
    }

    /// Adds a row and puts the keyboard focus in it.
    pub fn add_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.push_row("", "", window, cx);
        if let Some(row) = self.rows.last() {
            row.key.update(cx, |key, cx| key.focus(window, cx));
        }
    }

    fn pairs(&self, cx: &App) -> Vec<(String, String)> {
        let text = |input: &Entity<InputState>| input.read(cx).value().trim().to_string();
        self.rows
            .iter()
            .map(|row| (text(&row.key), text(&row.value)))
            .filter(|(key, value)| !key.is_empty() || !value.is_empty())
            .collect()
    }

    /// The text of the body, from its file or the editor, for a method that has one.
    fn body_text(&self, cx: &App) -> Option<Arc<str>> {
        if !self.has_body(cx) {
            return None;
        }
        if let Some(file) = &self.file {
            return Some(file.text.clone());
        }
        let text = self.body.read(cx).text(cx);
        (!text.trim().is_empty()).then(|| text.into())
    }

    /// Path and query of the form, or what is wrong with them.
    fn target(&self, cx: &App) -> Result<(String, Option<String>), SharedString> {
        let path = parse_path(&self.path_text).map_err(|e| path_error_text(&e))?;
        let query = raw_query(&self.pairs(cx))
            .map_err(|key| t!("console.query.refused", key = key).to_string())?;
        Ok((path, query))
    }

    /// Sends the form: masked body, unlock, confirmation, each stopping on failure. The body
    /// can be large, so its check and its copy run off the UI thread.
    pub fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.writing {
            return;
        }
        self.error = None;
        let (path, query) = match self.target(cx) {
            Ok(target) => target,
            Err(error) => {
                self.error = Some(error);
                return cx.notify();
            }
        };
        let method = self.method(cx);
        let text = self.body_text(cx);
        let file_mask = self.file.as_ref().map(|file| file.masked);
        let text_kind = self.kind.read(cx).selected_index(cx).map(|i| i.row) == Some(KIND_TEXT);
        let as_text = file_mask.is_some() || text_kind;
        let check = cx.background_spawn(async move {
            let text = text?;
            let masked = file_mask.unwrap_or_else(|| redact::text_has_mask(&text));
            Some(if masked {
                Err(())
            } else if as_text {
                Ok(RawBody::Text(text.to_string()))
            } else {
                Ok(RawBody::Json(text.to_string()))
            })
        });
        self.prepare_task = Some(cx.spawn_in(window, async move |this, cx| {
            let body = check.await.transpose();
            this.update_in(cx, |this, window, cx| match body {
                Ok(body) => {
                    let request = RawRequest {
                        method,
                        path,
                        query,
                        body,
                    };
                    this.send_checked(request, window, cx);
                }
                Err(()) => {
                    this.error = Some(t!("console.masked_body").to_string().into());
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn send_checked(&mut self, request: RawRequest, window: &mut Window, cx: &mut Context<Self>) {
        if READ_METHODS.contains(&request.method) {
            return self.go(request, window, cx);
        }
        let this = cx.entity().downgrade();
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                this.update(cx, |this, cx| this.confirm(request, window, cx))
                    .ok();
            });
        });
    }

    /// Asks what `raw_danger` wants, then sends.
    fn confirm(&mut self, request: RawRequest, window: &mut Window, cx: &mut Context<Self>) {
        let connection = self.connection.read(cx);
        let prod = connection.profile.environment.is_prod();
        let name = connection.profile.name.clone();
        let danger = raw_danger(&request.method, &request.path, prod);
        if danger == RawDanger::None {
            return self.go(request, window, cx);
        }
        let Some(client) = self.client(cx) else {
            self.error = Some(t!("status.not_connected").to_string().into());
            return cx.notify();
        };
        let query = request.query.as_ref().map(|q| format!("?{q}"));
        let target = format!(
            "{} {}{}",
            request.method,
            full_path(&client, &request.path),
            query.unwrap_or_default()
        );
        let title = t!("console.confirm.title", method = request.method).to_string();
        let action = t!("console.confirm.send").to_string();
        let this = cx.entity().downgrade();
        match danger {
            RawDanger::None => {}
            RawDanger::Medium => confirm::open(
                title,
                t!("console.confirm.body", target = target, connection = name).to_string(),
                action,
                move |window, cx| {
                    let request = request.clone();
                    this.update(cx, |this, cx| this.go(request, window, cx))
                        .ok();
                },
                window,
                cx,
            ),
            RawDanger::High { expected } => confirm_danger::open(
                DangerOptions {
                    title: title.into(),
                    consequences: vec![
                        target.into(),
                        t!("console.confirm.connection", connection = name)
                            .to_string()
                            .into(),
                    ],
                    expected: expected.into(),
                    action_label: action.into(),
                    prod,
                },
                move |window, cx| dialog_send(&this, request.clone(), window, cx),
                window,
                cx,
            ),
        }
    }

    /// Sends `request`: a read stays cancellable, a write runs on its own.
    fn go(&mut self, request: RawRequest, window: &mut Window, cx: &mut Context<Self>) {
        if READ_METHODS.contains(&request.method) {
            self.read_task = Some(self.start(request, window, cx));
        } else {
            self.start(request, window, cx).detach();
        }
    }

    /// Starts `request`. The task ends when the answer is taken in, also with this view gone,
    /// and its error is for `ConfirmDanger`, which keeps it in its dialog.
    fn start(
        &mut self,
        request: RawRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), SharedString>> {
        let Some(client) = self.client(cx) else {
            let text = SharedString::from(t!("status.not_connected").to_string());
            self.error = Some(text.clone());
            cx.notify();
            return Task::ready(Err(text));
        };
        let write = !READ_METHODS.contains(&request.method);
        self.read_task = None;
        self.generation += 1;
        let generation = self.generation;
        (self.reading, self.writing) = (!write, write);
        self.error = None;
        self.conceal(window, cx);

        let body = request.body.as_ref().map(RawBody::text);
        let entry = raw_entry(
            request.method.as_str(),
            &request.path,
            request.query.as_deref(),
            body,
        );
        let path = request.path.clone();
        let profile = self.connection.read(cx).profile.clone();
        let state = self.state.clone();
        let backend = cx.global::<SearchBackendGlobal>().0.clone();
        let call = backend.raw(client, request);
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let answer = call.await;
            let worked = match answer {
                Ok(mut answer) => Ok(cx
                    .background_spawn(async move {
                        let note = outcome_note(&path, answer.status, &answer.body);
                        let cut_from = cap_body(&mut answer.body, FULL_BODY_MAX);
                        let masked = redact::json(&answer.body);
                        Worked {
                            answer,
                            note,
                            cut_from,
                            masked,
                        }
                    })
                    .await),
                Err(e) => Err(e),
            };
            cx.update(|window, cx| {
                if worked.is_ok() {
                    let item = HistoryItem {
                        at: now(),
                        profile: profile.id,
                        entry,
                    };
                    state.update(cx, |s, cx| s.record_history(item, cx));
                }
                let error = worked
                    .as_ref()
                    .err()
                    .map(|e: &Error| SharedString::from(long_error_text(e, &profile)));
                this.update(cx, |this, cx| {
                    this.finished(generation, worked, error.clone(), window, cx);
                })
                .ok();
                error.map_or(Ok(()), Err)
            })
            .unwrap_or(Ok(()))
        })
    }

    fn finished(
        &mut self,
        generation: u64,
        worked: Result<Worked, Error>,
        error: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The user stopped waiting, or a newer request runs.
        if generation != self.generation {
            return;
        }
        (self.reading, self.writing) = (false, false);
        self.error = error;
        if let Ok(worked) = worked {
            let Worked {
                answer,
                note,
                cut_from,
                masked,
            } = worked;
            let masked: Arc<str> = masked.into();
            self.response
                .update(cx, |r, cx| r.set_body(masked.clone(), window, cx));
            self.answer = Some(Answer {
                status: answer.status,
                duration: answer.duration,
                headers: answer.headers,
                note,
                size: cut_from.unwrap_or(answer.body.len()),
                cut: cut_from.is_some(),
                text: answer.body.into(),
                masked,
            });
        }
        cx.notify();
    }

    /// Lets go of the running request without cancelling a write on the server.
    pub fn stop_waiting(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.read_task = None;
        (self.reading, self.writing) = (false, false);
        cx.notify();
    }

    /// Back to the masked answer.
    fn conceal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.revealed) {
            return;
        }
        if let Some(answer) = &self.answer {
            let masked = answer.masked.clone();
            self.response
                .update(cx, |r, cx| r.set_body(masked, window, cx));
        }
    }

    /// Shows the answer as the server sent it, or masks it again.
    pub fn toggle_reveal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.revealed {
            return self.conceal(window, cx);
        }
        if let Some(answer) = &self.answer {
            self.revealed = true;
            let text = answer.text.clone();
            self.response
                .update(cx, |r, cx| r.set_body(text, window, cx));
        }
        cx.notify();
    }

    /// Fills the form with `entry`, and sends nothing. A masked body stays masked.
    pub fn load(&mut self, entry: &HistoryEntry, window: &mut Window, cx: &mut Context<Self>) {
        let HistoryEntry::Raw {
            method,
            path,
            query,
            body,
            body_dropped,
        } = entry
        else {
            return;
        };
        self.set_method(method, window, cx);
        self.set_path(path, window, cx);
        self.rows.clear();
        for (key, value) in query_rows(query.as_deref().unwrap_or_default()) {
            self.push_row(&key, &value, window, cx);
        }
        self.file = None;
        let text = body.as_deref().unwrap_or_default();
        let is_text = !text.is_empty() && serde_json::from_str::<serde_json::Value>(text).is_err();
        let index = IndexPath::default().row(usize::from(is_text));
        self.kind
            .update(cx, |s, cx| s.set_selected_index(Some(index), window, cx));
        self.set_body(text, window, cx);
        self.error = None;
        self.note = body_dropped.then(|| t!("console.body_dropped").to_string().into());
        cx.notify();
    }

    /// Asks for a file to send as the body.
    pub fn pick_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("console.file.prompt").to_string().into()),
        });
        self.file_task = Some(cx.spawn_in(window, async move |this, cx| {
            let path = match picked.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Err(e)) => {
                    let error = t!("connect.error.picker", detail = e.to_string()).to_string();
                    this.update(cx, |this, cx| {
                        this.error = Some(error.into());
                        cx.notify();
                    })
                    .ok();
                    None
                }
                // Cancelled.
                Ok(Ok(None)) | Err(_) => None,
            };
            if let Some(path) = path {
                this.update_in(cx, |this, window, cx| this.attach(path, window, cx))
                    .ok();
            }
        }));
    }

    /// Reads `path` off the GPUI thread as the body. Refused inline above `RAW_FILE_MAX` or
    /// when it is not UTF-8.
    pub fn attach(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.error = None;
        let tokio = cx.global::<Tokio>().0.clone();
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        self.file_task = Some(cx.spawn_in(window, async move |this, cx| {
            let read = tokio.spawn_blocking(move || read_body(&path));
            let read = read.await.map_err(|e| e.to_string()).and_then(|r| r);
            this.update(cx, |this, cx| {
                match read {
                    Ok((text, masked)) => this.file = Some(FileBody { name, text, masked }),
                    Err(error) => this.error = Some(error.into()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn remove_file(&mut self, cx: &mut Context<Self>) {
        self.file = None;
        cx.notify();
    }

    /// Puts the request on the clipboard as curl. The key stays `$TYPESENSE_API_KEY`.
    pub fn copy_curl(&mut self, cx: &mut Context<Self>) {
        let (path, query) = match self.target(cx) {
            Ok(target) => target,
            Err(error) => {
                self.error = Some(error);
                return cx.notify();
            }
        };
        let Some(client) = self.client(cx) else {
            self.error = Some(t!("status.not_connected").to_string().into());
            return cx.notify();
        };
        let full = full_path(&client, &path);
        let method = self.method(cx);
        let file = self.file.as_ref().filter(|_| self.has_body(cx));
        let note = file.map(|f| t!("console.curl_file", name = f.name).to_string());
        let body = self.body_text(cx).filter(|_| file.is_none());
        let base = client.base_url().clone();
        // Masking the body can take long.
        let command = cx.background_spawn(async move {
            let command = curl::request(&base, &method, &full, query.as_deref(), body.as_deref());
            match note {
                Some(note) => format!(": {}\n{command}", curl::quote(&note)),
                None => command,
            }
        });
        cx.spawn(async move |_, cx| {
            let text = command.await;
            cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string(text)));
        })
        .detach();
    }

    /// The form as history and favorites keep it.
    fn entry(&self, cx: &App) -> Result<HistoryEntry, SharedString> {
        let (path, query) = self.target(cx)?;
        let body = self.body_text(cx);
        let method = self.method(cx);
        Ok(raw_entry(
            method.as_str(),
            &path,
            query.as_deref(),
            body.as_deref(),
        ))
    }

    /// Adds the request to the favorites, or takes it out.
    pub fn toggle_favorite(&mut self, cx: &mut Context<Self>) {
        let entry = match self.entry(cx) {
            Ok(entry) => entry,
            Err(error) => {
                self.error = Some(error);
                return cx.notify();
            }
        };
        let profile = self.profile_id(cx);
        let mut favorites = self.state.read(cx).saved.favorites.clone();
        match favorites
            .iter()
            .position(|f| f.profile == profile && f.entry == entry)
        {
            Some(index) => {
                favorites.remove(index);
            }
            None => favorites.push(HistoryItem {
                at: now(),
                profile,
                entry,
            }),
        }
        let save = self
            .state
            .update(cx, |s, cx| s.set_favorites(favorites, cx));
        self.report_save(save, cx);
    }

    /// Shows the failure of `save`, which `AppState` owns, when it comes.
    fn report_save(&mut self, save: Task<Result<(), String>>, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            if let Err(error) = save.await {
                this.update(cx, |this, cx| {
                    this.error = Some(error.into());
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
        cx.notify();
    }

    fn show_tab(&mut self, headers: bool, cx: &mut Context<Self>) {
        self.on_headers = headers;
        cx.notify();
    }

    /// What the toolbar and the panes disable or do not show now, for
    /// [`gated`](crate::actions::gated).
    pub fn gates(&self, cx: &App) -> [(bool, &'static dyn Action); 5] {
        let has_body = self.has_body(cx);
        [
            (
                parse_path(&self.path_text).is_err() || self.writing,
                &SendRequest,
            ),
            (!self.writing, &StopWaiting),
            (!has_body || self.file.is_some(), &AttachFile),
            (!has_body || self.file.is_none(), &RemoveBodyFile),
            (self.answer.is_none(), &RevealSecrets),
        ]
    }

    fn line(
        id: &'static str,
        text: SharedString,
        color: gpui_kit::Hsla,
    ) -> impl IntoElement + use<> {
        div()
            .id(id)
            .test_support()
            .aria_label(text.clone())
            .flex_none()
            .p_1()
            .text_sm()
            .text_color(color)
            .child(text)
    }

    fn toolbar(&mut self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let locked = self.connection.read(cx).writes_locked() && self.has_body(cx);
        let invalid = parse_path(&self.path_text).is_err();
        let kbd = |action: &dyn Action| Kbd::binding_for_action(action, Some(CONSOLE), window);
        h_flex()
            .flex_none()
            .gap_2()
            .p_1()
            .items_center()
            .child(
                div().w(px(110.)).flex_none().child(
                    Select::new(&self.method)
                        .id("console.method")
                        .small()
                        .accessibility_label(t!("console.method").to_string()),
                ),
            )
            .child(div().flex_1().min_w_0().child(self.path.clone()))
            .child(
                Button::new("console.send")
                    .primary()
                    .small()
                    .label(t!("console.send").to_string())
                    .children(kbd(&SendRequest))
                    // No `loading` on a read: it would refuse the click that replaces the read.
                    .loading(self.writing)
                    .disabled(invalid || self.writing)
                    .on_click(cx.listener(|this, _, window, cx| this.send(window, cx))),
            )
            .children(locked.then(|| env_badge::lock("console.send", false, cx)))
            .when(self.reading, |el| el.child(Spinner::new().small()))
            .when(self.writing, |el| {
                el.child(
                    Button::new("console.stop")
                        .ghost()
                        .small()
                        .label(t!("console.stop_waiting").to_string())
                        .children(Kbd::binding_for_action(&StopWaiting, Some(CONSOLE), window))
                        .on_click(cx.listener(|this, _, _, cx| this.stop_waiting(cx))),
                )
            })
            .child(
                Button::new("console.curl")
                    .ghost()
                    .small()
                    .label(t!("console.curl").to_string())
                    .children(kbd(&CopyCurl))
                    .on_click(cx.listener(|this, _, _, cx| this.copy_curl(cx))),
            )
            .child(self.list_button(window, cx))
            .child(
                Button::new("console.favorite")
                    .ghost()
                    .small()
                    .label(t!("console.favorite").to_string())
                    .children(kbd(&FavoriteRequest))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_favorite(cx))),
            )
    }

    fn request_pane(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (danger, muted) = (theme.danger, theme.muted_foreground);
        let bad_key = raw_query(&self.pairs(cx)).err();
        let rows = self.rows.iter().enumerate().map(|(i, row)| {
            h_flex()
                .gap_1()
                .child(
                    Input::new(&row.key)
                        .id(SharedString::from(format!("console.query.{i}.key")))
                        .small(),
                )
                .child(
                    Input::new(&row.value)
                        .id(SharedString::from(format!("console.query.{i}.value")))
                        .small(),
                )
        });
        let has_body = self.has_body(cx);
        let body_row = has_body.then(|| match &self.file {
            Some(file) => v_flex().flex_1().child(
                h_flex()
                    .gap_2()
                    .p_1()
                    .items_center()
                    .text_sm()
                    .child({
                        let text = SharedString::from(
                            t!(
                                "console.file.shown",
                                name = file.name,
                                size = size(file.text.len() as u64)
                            )
                            .to_string(),
                        );
                        div()
                            .id("console.file")
                            .test_support()
                            .aria_label(text.clone())
                            .flex_1()
                            .child(text)
                    })
                    .child(
                        Button::new("console.file.remove")
                            .ghost()
                            .small()
                            .label(t!("console.file.remove").to_string())
                            .children(Kbd::binding_for_action(
                                &RemoveBodyFile,
                                Some(CONSOLE),
                                window,
                            ))
                            .on_click(cx.listener(|this, _, _, cx| this.remove_file(cx))),
                    ),
            ),
            None => v_flex()
                .flex_1()
                .min_h_0()
                .child(
                    h_flex()
                        .flex_none()
                        .gap_2()
                        .p_1()
                        .items_center()
                        .text_sm()
                        .child(t!("console.body").to_string())
                        .child(
                            div().w(px(90.)).child(
                                Select::new(&self.kind)
                                    .id("console.kind")
                                    .small()
                                    .accessibility_label(t!("console.kind.label").to_string()),
                            ),
                        )
                        .child(
                            Button::new("console.file.pick")
                                .ghost()
                                .small()
                                .label(t!("console.file.pick").to_string())
                                .children(Kbd::binding_for_action(
                                    &AttachFile,
                                    Some(CONSOLE),
                                    window,
                                ))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.pick_file(window, cx);
                                })),
                        ),
                )
                .child(div().flex_1().min_h_0().child(self.body.clone())),
        });
        v_flex()
            .size_full()
            .child(
                v_flex()
                    .flex_none()
                    .gap_1()
                    .p_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .text_sm()
                            .child(t!("console.query.title").to_string())
                            .child(
                                Button::new("console.query.add")
                                    .ghost()
                                    .small()
                                    .label(t!("console.query.add").to_string())
                                    .children(Kbd::binding_for_action(
                                        &AddQueryRow,
                                        Some(CONSOLE),
                                        window,
                                    ))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.add_row(window, cx);
                                    })),
                            ),
                    )
                    .children(rows)
                    .children(bad_key.map(|key| {
                        let text = t!("console.query.refused", key = key).to_string();
                        Self::line("console.query_error", text.into(), danger)
                    })),
            )
            .children(body_row)
            .children(
                self.note
                    .clone()
                    .map(|note| Self::line("console.note", note, muted)),
            )
    }

    fn response_pane(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (danger, warning, foreground) = (theme.danger, theme.warning, theme.foreground);
        let status = self.answer.as_ref().map(|a| {
            let text = t!(
                "console.status",
                status = a.status,
                ms = a.duration.as_millis() as u64,
                size = size(a.size as u64)
            )
            .to_string();
            let color = if a.status >= 400 { danger } else { foreground };
            Self::line("console.status", text.into(), color)
        });
        let outcome = self.answer.as_ref().and_then(|a| a.note).map(|note| {
            let text = t!("console.outcome", failed = note.failed, total = note.total);
            Self::line("console.outcome", text.to_string().into(), warning)
        });
        let cut_note = self.answer.as_ref().filter(|a| a.cut).map(|a| {
            let text = t!("console.cut", total = size(a.size as u64)).to_string();
            Self::line("console.cut", text.into(), warning)
        });
        let answer = self.answer.as_ref();
        let headers = answer.into_iter().flat_map(|a| &a.headers).map(|(k, v)| {
            h_flex()
                .gap_2()
                .text_sm()
                .child(div().flex_none().child(k.clone()))
                .child(div().min_w_0().child(v.clone()))
        });
        let body_tab = t!("console.tab.body").to_string();
        let headers_tab = t!("console.tab.headers").to_string();
        let reveal = t!(if self.revealed {
            "console.conceal"
        } else {
            "console.reveal"
        })
        .to_string();
        v_flex()
            .size_full()
            .children(status)
            .children(outcome)
            .children(cut_note)
            .child(
                h_flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .child(
                        TabBar::new("console.tabs")
                            .underline()
                            .selected_index(usize::from(self.on_headers))
                            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                                this.show_tab(*ix == 1, cx);
                            }))
                            .child(view_tab(
                                "console.tab.body",
                                body_tab.into(),
                                &ShowResponseBody,
                                CONSOLE,
                                window,
                            ))
                            .child(view_tab(
                                "console.tab.headers",
                                headers_tab.into(),
                                &ShowResponseHeaders,
                                CONSOLE,
                                window,
                            )),
                    )
                    .child(
                        Button::new("console.reveal")
                            .ghost()
                            .small()
                            .label(reveal)
                            .children(Kbd::binding_for_action(
                                &RevealSecrets,
                                Some(CONSOLE),
                                window,
                            ))
                            .disabled(self.answer.is_none())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_reveal(window, cx);
                            })),
                    ),
            )
            .child(div().flex_1().min_h_0().map(|el| {
                if self.on_headers {
                    el.id("console.headers")
                        .test_support()
                        .overflow_y_scroll()
                        .p_1()
                        .children(headers.collect::<Vec<_>>())
                        .into_any_element()
                } else {
                    el.child(self.response.clone()).into_any_element()
                }
            }))
    }
}

/// `path` below the node, for curl and the confirmations.
fn full_path(client: &Client, path: &str) -> String {
    let base = client.base_url();
    base.join(path)
        .map_or_else(|_| path.to_string(), |url| url.path().to_string())
}

/// Sends `request` from `ConfirmDanger`, whose "Stop waiting" leaves it running.
fn dialog_send(
    view: &WeakEntity<ConsoleView>,
    request: RawRequest,
    window: &mut Window,
    cx: &mut App,
) -> Task<Result<(), SharedString>> {
    view.update(cx, |this, cx| this.start(request, window, cx))
        .unwrap_or_else(|_| Task::ready(Ok(())))
}

/// The text of the file at `path` and whether it holds a mask, or why it cannot be a body.
fn read_body(path: &std::path::Path) -> Result<(Arc<str>, bool), String> {
    let shown = path.display().to_string();
    let failed = |e: std::io::Error| {
        t!(
            "console.file.unreadable",
            path = shown,
            detail = e.to_string()
        )
        .to_string()
    };
    let bytes = std::fs::metadata(path).map_err(failed)?.len();
    if bytes > RAW_FILE_MAX {
        return Err(t!("console.file.too_large", size = size(bytes)).to_string());
    }
    let data = std::fs::read(path).map_err(failed)?;
    let text = String::from_utf8(data).map_err(|_| t!("console.file.not_utf8").to_string())?;
    let masked = redact::text_has_mask(&text);
    Ok((text.into(), masked))
}

impl Focusable for ConsoleView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ConsoleView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let danger = cx.theme().danger;
        let path_error = match parse_path(&self.path_text) {
            // Nothing typed yet is no mistake; Send stays disabled.
            Err(PathError::Empty) | Ok(_) => None,
            Err(error) => Some(path_error_text(&error)),
        };
        let toolbar = self.toolbar(window, cx);
        let request = self.request_pane(window, cx);
        let response = self.response_pane(window, cx);
        v_flex()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &SendRequest, window, cx| this.send(window, cx)))
            .on_action(cx.listener(|this, _: &AddQueryRow, window, cx| this.add_row(window, cx)))
            .on_action(cx.listener(|this, _: &AttachFile, window, cx| this.pick_file(window, cx)))
            .on_action(cx.listener(|this, _: &FavoriteRequest, _, cx| this.toggle_favorite(cx)))
            .on_action(cx.listener(|this, _: &RevealSecrets, window, cx| {
                this.toggle_reveal(window, cx);
            }))
            .on_action(cx.listener(|this, _: &CopyCurl, _, cx| this.copy_curl(cx)))
            .on_action(cx.listener(|this, _: &StopWaiting, _, cx| this.stop_waiting(cx)))
            .on_action(cx.listener(|this, _: &RemoveBodyFile, _, cx| this.remove_file(cx)))
            .on_action(cx.listener(|this, _: &ShowResponseBody, _, cx| this.show_tab(false, cx)))
            .on_action(cx.listener(|this, _: &ShowResponseHeaders, _, cx| {
                this.show_tab(true, cx);
            }))
            .on_action(cx.listener(|this, _: &PreviousChunk, window, cx| {
                this.response
                    .update(cx, |r, cx| r.previous_chunk(window, cx));
            }))
            .on_action(cx.listener(|this, _: &NextChunk, window, cx| {
                this.response.update(cx, |r, cx| r.next_chunk(window, cx));
            }))
            .on_action(cx.listener(|this, _: &OpenHistory, window, cx| {
                this.toggle_list(window, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectUp, _, cx| this.step_list(-1, cx)))
            .on_action(cx.listener(|this, _: &SelectDown, _, cx| this.step_list(1, cx)))
            .on_action(cx.listener(|this, _: &LoadQuery, window, cx| {
                this.load_selected(window, cx);
            }))
            .size_full()
            .child(toolbar)
            .children(path_error.map(|text| Self::line("console.path_error", text.into(), danger)))
            .children(
                self.error
                    .clone()
                    .map(|text| Self::line("console.error", text, danger)),
            )
            .child(
                div().flex_1().min_h_0().child(
                    h_resizable("console.split")
                        .child(
                            resizable_panel()
                                .size(px(REQUEST_WIDTH))
                                .border_r_1()
                                .border_color(cx.theme().border)
                                .child(request),
                        )
                        .child(resizable_panel().child(response)),
                ),
            )
    }
}
