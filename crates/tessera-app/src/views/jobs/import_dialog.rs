// SPDX-License-Identifier: GPL-3.0-or-later

//! The dialog that imports documents from a file or the clipboard into a collection. It sends
//! nothing until Start; the job then runs on its own, whatever happens to this dialog.

use std::cell::Cell;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dialog::Cancel;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IndexPath, Selectable as _, Sizable as _, WindowExt as _,
    h_flex, searchable_list::SearchableListItem, v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, ExternalPaths, FocusHandle, IntoElement,
    ParentElement as _, PathPromptOptions, Render, SharedString, Styled as _, Subscription, Task,
    Window, div, prelude::*, px,
};
use rust_i18n::t;
use serde_json::Value;
use tessera_client::{Collection, DirtyValues, ImportAction, ImportParams};
use tessera_core::documents::export::{Danger, danger_level};
use tessera_core::schema::{FieldType, SchemaDraft, Severity, validate};
use tessera_core::transfer::import::ImportJob;
use tessera_core::transfer::infer::{InferNote, collection_name_from};
use tessera_core::transfer::preview::{Preview, PreviewError, PreviewRow};
use tessera_core::transfer::source::{CsvOptions, Format, ReadError, ReadOptions, Source, sniff};
use tessera_core::transfer::{CLIPBOARD_LIMIT, DEFAULT_BATCH, MAX_BATCH, MAX_DOC_BYTES};
use tessera_widgets::confirm;
use tessera_widgets::confirm_danger::{self, DangerOptions};

use super::super::env_badge;
use super::super::overview::grouped;
use super::super::request_detail::size;
use super::super::root::RootView;
use super::super::schema_dialog::SchemaDialog;
use super::{infer_notes, note, note_owned};
use crate::actions::{EditImportSchema, IMPORT_DIALOG, PasteImport, PickImportFile, StartTransfer};
use crate::collections::CollectionsBackendGlobal;
use crate::connection::{ConnectionState, long_error_text};
use crate::jobs::JobsGlobal;
use crate::runtime::Tokio;
use crate::state::AppState;
use crate::transfer::{FileStamp, TransferBackendGlobal, file_stamp, read_error_text};

const WIDTH: f32 = 900.;
/// A change settles for this long before the preview reruns.
const PREVIEW_DELAY: Duration = Duration::from_millis(150);
/// Enough of a file to tell its format.
const HEAD_BYTES: usize = 4096;
const PREVIEW_HEIGHT: f32 = 180.;

const FORMATS: [(Format, &str); 3] = [
    (Format::Jsonl, "jsonl"),
    (Format::JsonArray, "json"),
    (Format::Csv, "csv"),
];
const DELIMITERS: [(u8, &str); 4] = [
    (b',', "comma"),
    (b';', "semicolon"),
    (b'\t', "tab"),
    (b'|', "pipe"),
];
const ACTIONS: [ImportAction; 4] = [
    ImportAction::Create,
    ImportAction::Upsert,
    ImportAction::Update,
    ImportAction::Emplace,
];
const DIRTY: [DirtyValues; 4] = [
    DirtyValues::CoerceOrReject,
    DirtyValues::CoerceOrDrop,
    DirtyValues::Drop,
    DirtyValues::Reject,
];

/// What the select offers.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// An existing collection.
    Collection(String),
    /// A collection that Start creates.
    New,
}

#[derive(Clone, PartialEq)]
struct TargetItem(Target);

impl SearchableListItem for TargetItem {
    type Value = Target;

    fn title(&self) -> SharedString {
        match &self.0 {
            Target::Collection(name) => name.clone().into(),
            Target::New => t!("transfer.import.new_collection").to_string().into(),
        }
    }

    fn value(&self) -> &Target {
        &self.0
    }
}

/// A collection Start would create. It exists only here until then.
#[derive(Default)]
struct NewTarget {
    /// `None` until the inference answered.
    draft: Option<SchemaDraft>,
    notes: Vec<InferNote>,
    sampled: u64,
    error: Option<SharedString>,
    task: Option<Task<()>>,
}

/// What will be read.
#[derive(Clone)]
struct Loaded {
    source: Source,
    /// The file name; `None` for the clipboard.
    name: Option<SharedString>,
    /// The file as it was read; `None` for the clipboard.
    stamp: Option<FileStamp>,
    /// The line above the options.
    shown: SharedString,
}

/// Why a path cannot be a source.
pub(crate) enum Inspect {
    NotAFile,
    Io(String),
}

/// The size and first bytes of `path`.
pub(crate) fn inspect(path: &Path) -> Result<(u64, Vec<u8>), Inspect> {
    let meta = std::fs::metadata(path).map_err(|e| Inspect::Io(e.to_string()))?;
    if !meta.is_file() {
        return Err(Inspect::NotAFile);
    }
    let file = std::fs::File::open(path).map_err(|e| Inspect::Io(e.to_string()))?;
    let mut head = Vec::with_capacity(HEAD_BYTES);
    file.take(HEAD_BYTES as u64)
        .read_to_end(&mut head)
        .map_err(|e| Inspect::Io(e.to_string()))?;
    Ok((meta.len(), head))
}

/// The line above the options for a file.
fn file_shown(name: Option<&SharedString>, bytes: u64) -> SharedString {
    let name = name.cloned().unwrap_or_default();
    t!("transfer.import.file", name = name, size = size(bytes))
        .to_string()
        .into()
}

/// `n` documents per batch, if `text` is a size the import takes.
fn batch_size(text: &str) -> Option<u32> {
    text.trim()
        .parse()
        .ok()
        .filter(|n| (1..=MAX_BATCH).contains(n))
}

/// The dialog's state. Closing it stops a preview; a started job is not its own.
pub struct DocumentImportDialog {
    state: Entity<AppState>,
    connection: Entity<ConnectionState>,
    focus_handle: FocusHandle,
    source: Option<Loaded>,
    source_error: Option<SharedString>,
    source_task: Option<Task<()>>,
    /// Start found the file changed, and the estimate is new.
    file_changed: bool,
    /// Compares the file before Start goes on.
    check_task: Option<Task<()>>,
    format: Format,
    csv: CsvOptions,
    target: Entity<SelectState<Vec<TargetItem>>>,
    /// The names `target` holds; `None` until it is filled.
    offered: Option<Vec<String>>,
    /// The chosen collection.
    collection: Option<String>,
    /// Set while "New collection…" is chosen, instead of `collection`.
    new: Option<NewTarget>,
    new_name: Entity<InputState>,
    create_error: Option<SharedString>,
    creating: bool,
    /// Dropping it cancels the create.
    create_task: Option<Task<()>>,
    action: ImportAction,
    dirty: DirtyValues,
    batch: Entity<InputState>,
    /// `None` until an answer for the current options arrived.
    preview: Option<Result<Preview, ReadError>>,
    /// Replaced by each change, which drops a stale answer.
    preview_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl DocumentImportDialog {
    /// Opens the dialog on `connection`, with `target` chosen and `source` read, if given.
    pub fn open(
        state: Entity<AppState>,
        connection: Entity<ConnectionState>,
        target: Option<String>,
        source: Option<Source>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let profile = connection.read(cx).profile.clone();
        let view = cx.new(|cx| Self::new(state, connection, target, source, window, cx));
        let focus = view.read(cx).focus_handle.clone();
        let first_build = Cell::new(true);
        let dialog_view = view.clone();
        window.open_dialog(cx, move |dialog, window, cx| {
            if first_build.replace(false) {
                focus.focus(window, cx);
            }
            let cancel = dialog_view.clone();
            dialog
                .title(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(t!("transfer.import.title").to_string())
                        .child(
                            div()
                                .id("transfer.import.connection")
                                .test_support()
                                .aria_label(SharedString::from(profile.name.clone()))
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(profile.name.clone()),
                        )
                        .child(
                            div()
                                .id("transfer.import.env")
                                .test_support()
                                .aria_label(SharedString::from(env_badge::label(
                                    &profile.environment,
                                )))
                                .child(env_badge::render(&profile.environment, cx)),
                        ),
                )
                .w(px(WIDTH))
                // Enter would close the dialog; Start is ⌘↩ / Ctrl+↩ instead.
                .on_ok(|_, _, _| false)
                // A create in flight finishes first, so its failure lands here.
                .on_cancel(move |_, _, cx| !cancel.read(cx).creating)
                .child(dialog_view.clone())
        });
        view
    }

    fn new(
        state: Entity<AppState>,
        connection: Entity<ConnectionState>,
        target: Option<String>,
        source: Option<Source>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let select = cx.new(|cx| SelectState::new(Vec::new(), None, window, cx).searchable(true));
        let batch = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_value(DEFAULT_BATCH.to_string(), window, cx);
            input
        });
        let new_name = cx.new(|cx| InputState::new(window, cx));
        let subscriptions = vec![
            cx.subscribe(&new_name, |this, _, _: &InputEvent, cx| {
                this.name_changed(cx)
            }),
            cx.observe_in(&connection, window, |this, _, window, cx| {
                this.collections_changed(window, cx);
            }),
            cx.subscribe_in(
                &select,
                window,
                |this, _, event: &SelectEvent<Vec<TargetItem>>, window, cx| match event {
                    SelectEvent::Confirm(Some(Target::Collection(name))) => {
                        this.pick_target(name.clone(), cx);
                    }
                    SelectEvent::Confirm(Some(Target::New)) => this.pick_new(window, cx),
                    _ => {}
                },
            ),
            cx.subscribe(&batch, |_, _, _: &InputEvent, cx| cx.notify()),
        ];
        let mut this = Self {
            state,
            connection,
            focus_handle: cx.focus_handle(),
            source: None,
            source_error: None,
            source_task: None,
            file_changed: false,
            check_task: None,
            format: Format::Jsonl,
            csv: CsvOptions::default(),
            target: select,
            offered: None,
            collection: target,
            new: None,
            new_name,
            create_error: None,
            creating: false,
            create_task: None,
            action: ImportAction::Create,
            dirty: DirtyValues::CoerceOrReject,
            batch,
            preview: None,
            preview_task: None,
            _subscriptions: subscriptions,
        };
        if !this.list_loaded(cx) {
            this.connection
                .update(cx, |c, cx| c.load_collection_list(cx));
        }
        this.collections_changed(window, cx);
        match source {
            Some(Source::File(path)) => this.read_file(path, cx),
            Some(Source::Bytes(bytes)) => this.read_bytes(bytes, cx),
            None => {}
        }
        this
    }

    fn list_loaded(&self, cx: &App) -> bool {
        let cache = &self.connection.read(cx).collection_cache;
        cache.collections.is_some()
    }

    /// The names of the connection's collections.
    fn names(&self, cx: &App) -> Vec<String> {
        let cache = &self.connection.read(cx).collection_cache;
        let all = cache.collections.iter().flatten();
        all.map(|c| c.name.clone()).collect()
    }

    /// Fills the select from the list, which may arrive after the dialog opened. The first
    /// call fills it whatever the list holds: "New collection…" needs no list.
    fn collections_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let names = self.names(cx);
        let loaded = self.list_loaded(cx);
        // The connection also notifies for changes that leave the list as it was, and a
        // reload drops the list for a moment.
        if (self.offered.as_ref()).is_some_and(|offered| !loaded || *offered == names) {
            cx.notify();
            return;
        }
        self.offered = Some(names.clone());
        // A collection that went away is no target; the list keeps it out of the job too.
        if loaded {
            self.collection = self.collection.take().filter(|name| names.contains(name));
        }
        let items: Vec<_> = names
            .iter()
            .cloned()
            .map(Target::Collection)
            .chain([Target::New])
            .map(TargetItem)
            .collect();
        let row = match &self.new {
            Some(_) => Some(names.len()),
            None => self
                .collection
                .as_ref()
                .and_then(|name| names.iter().position(|n| n == name)),
        };
        self.target.update(cx, |select, cx| {
            select.set_items(items, window, cx);
            let index = row.map(|row| IndexPath::default().row(row));
            select.set_selected_index(index, window, cx);
        });
        if self.new.is_none() {
            self.schedule_preview(cx);
        }
    }

    /// Chooses the collection to import into.
    pub fn pick_target(&mut self, name: String, cx: &mut Context<Self>) {
        self.collection = Some(name);
        self.new = None;
        self.schedule_preview(cx);
    }

    /// Chooses a new collection: proposes a name and a schema. Nothing reaches the server.
    pub fn pick_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.collection = None;
        self.new = Some(NewTarget::default());
        if matches!(self.action, ImportAction::Update | ImportAction::Emplace) {
            self.action = ImportAction::Create;
        }
        let name = match self.source.as_ref().map(|loaded| &loaded.source) {
            Some(Source::File(path)) => collection_name_from(path),
            _ => "imported".to_string(),
        };
        self.new_name
            .update(cx, |input, cx| input.set_value(name, window, cx));
        self.target.update(cx, |select, cx| {
            select.set_selected_value(&Target::New, window, cx);
        });
        self.infer(cx);
    }

    /// What the select shows as chosen.
    pub fn chosen(&self, cx: &App) -> Option<Target> {
        self.target.read(cx).selected_value().cloned()
    }

    /// The name typed for the new collection.
    pub fn new_name(&self, cx: &App) -> String {
        self.new_name.read(cx).value().to_string()
    }

    fn name_changed(&mut self, cx: &mut Context<Self>) {
        let name = self.new_name(cx);
        if let Some(draft) = self.new.as_mut().and_then(|new| new.draft.as_mut())
            && draft.name != name
        {
            draft.name = name;
            cx.notify();
        }
    }

    /// Proposes a schema from the current source and options.
    fn infer(&mut self, cx: &mut Context<Self>) {
        self.preview = None;
        self.preview_task = None;
        cx.notify();
        let (Some(loaded), Some(new)) = (&self.source, &mut self.new) else {
            return;
        };
        let source = loaded.source.clone();
        let options = ReadOptions {
            format: self.format,
            csv: self.csv.clone(),
            fields: Vec::new(),
        };
        new.draft = None;
        new.error = None;
        let backend = cx.global::<TransferBackendGlobal>().0.clone();
        new.task = Some(cx.spawn(async move |this, cx| {
            let answer = backend.infer(source, options).await;
            this.update(cx, |this, cx| {
                let name = this.new_name(cx);
                let Some(new) = &mut this.new else {
                    return;
                };
                match answer {
                    Ok(inferred) => {
                        new.draft = Some(inferred.to_draft(&name));
                        new.sampled = inferred.sampled;
                        new.notes = inferred.notes;
                    }
                    Err(e) => new.error = Some(read_error_text(&e).into()),
                }
                this.schedule_preview(cx);
            })
            .ok();
        }));
    }

    /// Reads the source again for changed options.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.new.is_some() {
            self.infer(cx);
        } else {
            self.schedule_preview(cx);
        }
    }

    fn use_geopoint(&mut self, field: &str, cx: &mut Context<Self>) {
        let Some(new) = &mut self.new else {
            return;
        };
        let drafted = new.draft.iter_mut().flat_map(|d| d.fields.iter_mut());
        for draft in drafted.filter(|f| f.name == field) {
            draft.r#type = FieldType::Geopoint;
        }
        new.notes
            .retain(|note| !matches!(note, InferNote::MaybeGeopoint(f) if f == field));
        self.schedule_preview(cx);
    }

    fn edit_schema(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.new.as_ref().and_then(|new| new.draft.clone()) else {
            return;
        };
        let dialog = cx.entity().downgrade();
        SchemaDialog::open_pick(
            self.state.clone(),
            self.connection.clone(),
            draft,
            move |json, window, cx| {
                dialog
                    .update(cx, |d, cx| d.schema_picked(&json, window, cx))
                    .ok();
            },
            window,
            cx,
        );
    }

    /// Takes the schema dialog's result; it validated it.
    fn schema_picked(&mut self, json: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(new), Ok(draft)) = (&mut self.new, SchemaDraft::from_json(&json.to_string()))
        else {
            return;
        };
        new.notes.retain(|note| match note {
            InferNote::MaybeGeopoint(field) => draft
                .fields
                .iter()
                .any(|f| f.name == *field && f.r#type != FieldType::Geopoint),
            _ => true,
        });
        let name = draft.name.clone();
        new.draft = Some(draft);
        self.new_name
            .update(cx, |input, cx| input.set_value(name, window, cx));
        self.schedule_preview(cx);
    }

    /// The schema of the chosen collection, or the draft of a new one if it decodes as one.
    fn schema(&self, cx: &App) -> Option<Collection> {
        if let Some(new) = &self.new {
            let json = new.draft.as_ref()?.to_json();
            return serde_json::from_value(json).ok();
        }
        let name = self.collection.as_ref()?;
        let cache = &self.connection.read(cx).collection_cache;
        cache
            .collections
            .iter()
            .flatten()
            .find(|c| c.name == *name)
            .cloned()
    }

    fn options(&self, schema: Option<&Collection>) -> ReadOptions {
        ReadOptions {
            format: self.format,
            csv: self.csv.clone(),
            fields: schema.map_or_else(Vec::new, |c| {
                let fields = c.fields.iter().filter(|f| !f.name.contains('*'));
                fields.map(|f| (f.name.clone(), f.r#type.clone())).collect()
            }),
        }
    }

    /// Reruns the preview once the options stood still for a moment.
    fn schedule_preview(&mut self, cx: &mut Context<Self>) {
        self.preview = None;
        self.preview_task = None;
        cx.notify();
        let Some(loaded) = &self.source else {
            return;
        };
        let source = loaded.source.clone();
        let schema = self.schema(cx);
        let options = self.options(schema.as_ref());
        let backend = cx.global::<TransferBackendGlobal>().0.clone();
        self.preview_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PREVIEW_DELAY).await;
            let answer = backend.preview(source, options, schema).await;
            this.update(cx, |this, cx| {
                this.preview = Some(answer);
                cx.notify();
            })
            .ok();
        }));
    }

    fn fail_source(&mut self, text: String, cx: &mut Context<Self>) {
        self.source = None;
        self.source_task = None;
        self.source_error = Some(text.into());
        self.schedule_preview(cx);
    }

    /// Takes `path` as the source: reads its size and head off the GPUI thread, then sniffs.
    pub fn read_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let tokio = cx.global::<Tokio>().0.clone();
        self.source_error = None;
        self.file_changed = false;
        let shown = path.clone();
        let read = tokio.spawn_blocking(move || {
            let stamp = file_stamp(&shown).ok();
            inspect(&shown).map(|(bytes, head)| (bytes, head, stamp))
        });
        self.source_task = Some(cx.spawn(async move |this, cx| {
            let read = read.await;
            this.update(cx, |this, cx| match read {
                Ok(Ok((bytes, head, stamp))) => {
                    let name: Option<SharedString> = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned().into());
                    let (format, csv) = sniff(Some(&path), &head);
                    (this.format, this.csv) = (format, csv);
                    this.source = Some(Loaded {
                        shown: file_shown(name.as_ref(), bytes),
                        source: Source::File(path),
                        name,
                        stamp,
                    });
                    this.refresh(cx);
                }
                Ok(Err(Inspect::NotAFile)) => {
                    let text = t!("transfer.import.not_a_file").to_string();
                    this.fail_source(text, cx);
                }
                Ok(Err(Inspect::Io(detail))) => {
                    let text = read_error_text(&ReadError::Io(detail));
                    this.fail_source(text, cx);
                }
                Err(e) => this.fail_source(read_error_text(&ReadError::Io(e.to_string())), cx),
            })
            .ok();
        }));
        cx.notify();
    }

    fn read_bytes(&mut self, bytes: Arc<[u8]>, cx: &mut Context<Self>) {
        let (format, csv) = sniff(None, &bytes[..bytes.len().min(HEAD_BYTES)]);
        (self.format, self.csv) = (format, csv);
        // A file still being read must not replace this.
        self.source_task = None;
        self.source_error = None;
        self.file_changed = false;
        self.source = Some(Loaded {
            shown: t!("transfer.import.clipboard", size = size(bytes.len() as u64))
                .to_string()
                .into(),
            source: Source::Bytes(bytes),
            name: None,
            stamp: None,
        });
        self.refresh(cx);
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        let text = cx.read_from_clipboard().and_then(|c| c.text());
        match text.filter(|text| !text.is_empty()) {
            None => self.fail_source(t!("transfer.import.clipboard_empty").to_string(), cx),
            Some(text) if text.len() > CLIPBOARD_LIMIT => {
                let held = size(text.len() as u64);
                let text = t!("transfer.import.clipboard_too_large", size = held).to_string();
                self.fail_source(text, cx);
            }
            Some(text) => self.read_bytes(Arc::from(text.into_bytes()), cx),
        }
    }

    fn choose_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("transfer.import.choose").to_string().into()),
        });
        self.source_task = Some(cx.spawn_in(window, async move |this, cx| {
            match picked.await {
                Ok(Ok(Some(paths))) => {
                    if let Some(path) = paths.into_iter().next() {
                        this.update(cx, |this, cx| this.read_file(path, cx)).ok();
                    }
                }
                Ok(Err(e)) => {
                    let text = t!("connect.error.picker", detail = e.to_string()).to_string();
                    this.update(cx, |this, cx| this.fail_source(text, cx)).ok();
                }
                // Cancelled.
                Ok(Ok(None)) | Err(_) => {}
            }
        }));
    }

    fn set_format(&mut self, format: Format, cx: &mut Context<Self>) {
        self.format = format;
        self.refresh(cx);
    }

    fn batch_value(&self, cx: &App) -> Option<u32> {
        batch_size(&self.batch.read(cx).value())
    }

    /// What is wrong with the draft of the new collection, by the rules of the schema dialog.
    fn draft_errors(&self) -> Vec<String> {
        let draft = self.new.as_ref().and_then(|new| new.draft.as_ref());
        let issues = draft.map(validate).unwrap_or_default();
        issues
            .iter()
            .filter(|issue| issue.severity == Severity::Error)
            .map(|issue| t!(issue.key).to_string())
            .collect()
    }

    /// What Start would run, when everything is valid.
    fn job(&self, cx: &App) -> Option<ImportJob> {
        let source = self.source.as_ref()?.source.clone();
        let collection = match &self.new {
            Some(new) if self.draft_errors().is_empty() => new.draft.as_ref()?.name.clone(),
            Some(_) => return None,
            None => self.collection.clone()?,
        };
        let batch_size = self.batch_value(cx)?;
        matches!(self.preview, Some(Ok(_))).then(|| ImportJob {
            source,
            options: self.options(self.schema(cx).as_ref()),
            collection,
            params: ImportParams {
                action: self.action,
                batch_size,
                dirty_values: self.dirty,
            },
        })
    }

    /// Compares a file with what was read off the GPUI thread, then goes on. A file that
    /// changed stays in the dialog with a new estimate, so the danger level follows it.
    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A second create would drop the first in flight.
        if self.creating || self.job(cx).is_none() {
            return;
        }
        self.file_changed = false;
        cx.notify();
        let file = self
            .source
            .as_ref()
            .and_then(|loaded| match &loaded.source {
                Source::File(path) => Some((path.clone(), loaded.stamp)),
                Source::Bytes(_) => None,
            });
        let Some((path, read)) = file else {
            return self.unlock_and_confirm(window, cx);
        };
        let check = cx.global::<TransferBackendGlobal>().0.file_stamp(path);
        self.check_task = Some(cx.spawn_in(window, async move |this, cx| {
            let now = check.await;
            this.update_in(cx, |this, window, cx| match now {
                Ok(now) if Some(now) == read => this.unlock_and_confirm(window, cx),
                Ok(now) => this.source_stale(now, cx),
                Err(e) => this.fail_source(read_error_text(&e), cx),
            })
            .ok();
        }));
    }

    /// Takes the file as it is now and estimates it again. A draft keeps the user's edits.
    fn source_stale(&mut self, now: FileStamp, cx: &mut Context<Self>) {
        if let Some(loaded) = &mut self.source {
            loaded.stamp = Some(now);
            loaded.shown = file_shown(loaded.name.as_ref(), now.0);
        }
        self.file_changed = true;
        self.schedule_preview(cx);
    }

    /// Validates, asks to unlock a read-only connection, then confirms.
    fn unlock_and_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(job) = self.job(cx) else {
            return;
        };
        let dialog = cx.entity();
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                dialog.update(cx, |d, cx| d.confirm(job, window, cx));
            });
        });
    }

    /// What the job will do, one sentence each.
    fn consequences(&self, job: &ImportJob, overwrites: bool, cx: &App) -> Vec<SharedString> {
        let docs = match &self.preview {
            Some(Ok(preview)) => grouped(preview.estimated_docs),
            _ => String::new(),
        };
        let name = self.source.as_ref().and_then(|l| l.name.clone());
        let connection = self.connection.read(cx).profile.name.clone();
        let collection = &job.collection;
        let mut lines = vec![
            t!(
                "transfer.import.confirm.consequence.target",
                collection = collection,
                connection = connection
            ),
            match name {
                Some(name) => t!(
                    "transfer.import.confirm.consequence.source",
                    name = name,
                    docs = docs
                ),
                None => t!(
                    "transfer.import.confirm.consequence.source_clipboard",
                    docs = docs
                ),
            },
        ];
        if overwrites {
            lines.push(t!("transfer.import.confirm.consequence.overwrite"));
        }
        if let Some(draft) = self.new.as_ref().and_then(|new| new.draft.as_ref()) {
            lines.push(t!(
                "transfer.import.confirm.consequence.new",
                collection = collection,
                fields = draft.fields.len()
            ));
        }
        lines.push(t!("transfer.import.confirm.consequence.stay"));
        lines.into_iter().map(|l| l.to_string().into()).collect()
    }

    /// Asks as D49 says: `create` and a new collection only on prod, an overwriting action
    /// always, by name on prod or above 1,000 estimated documents.
    fn confirm(&mut self, job: ImportJob, window: &mut Window, cx: &mut Context<Self>) {
        let prod = self.connection.read(cx).profile.environment.is_prod();
        let estimated = match &self.preview {
            Some(Ok(preview)) => preview.estimated_docs,
            _ => 0,
        };
        let overwrites = self.new.is_none() && job.params.action != ImportAction::Create;
        let danger = if overwrites {
            Some(danger_level(estimated, prod))
        } else {
            prod.then_some(Danger::Medium)
        };
        let Some(danger) = danger else {
            return self.launch(job, window, cx);
        };
        let consequences = self.consequences(&job, overwrites, cx);
        let dialog = cx.entity();
        let title = t!("transfer.import.confirm.title").to_string();
        let action = t!("transfer.import.confirm.action").to_string();
        match danger {
            Danger::Medium => {
                let body = consequences
                    .iter()
                    .map(SharedString::as_ref)
                    .collect::<Vec<_>>()
                    .join("\n");
                let on_confirm = move |window: &mut Window, cx: &mut App| {
                    let (dialog, job) = (dialog.clone(), job.clone());
                    // After the confirmation has closed, whichever way it was accepted.
                    window.defer(cx, move |window, cx| {
                        dialog.update(cx, |d, cx| d.launch(job, window, cx));
                    });
                };
                confirm::open(title, body, action, on_confirm, window, cx);
            }
            // Never a new collection, so nothing can fail here. `run` closes this dialog, and
            // the Ok below closes the one beneath it.
            Danger::High => confirm_danger::open(
                DangerOptions {
                    title: title.into(),
                    consequences,
                    expected: job.collection.clone().into(),
                    action_label: action.into(),
                    prod,
                },
                move |window, cx| {
                    let job = job.clone();
                    dialog.update(cx, |d, cx| d.launch(job, window, cx));
                    Task::ready(Ok(()))
                },
                window,
                cx,
            ),
        }
    }

    /// Creates a new collection, then runs the job. A failed create stays in the dialog.
    fn launch(&mut self, job: ImportJob, window: &mut Window, cx: &mut Context<Self>) {
        let schema = self.new.as_ref().and_then(|new| new.draft.as_ref());
        let Some(schema) = schema.map(SchemaDraft::to_json) else {
            return self.run(job, window, cx);
        };
        let conn = self.connection.read(cx);
        let profile = conn.profile.clone();
        let Some(client) = conn.connected.as_ref().map(|c| c.client.clone()) else {
            self.create_error = Some(t!("status.not_connected").to_string().into());
            return cx.notify();
        };
        let create = cx
            .global::<CollectionsBackendGlobal>()
            .0
            .create(client, schema);
        self.creating = true;
        self.create_error = None;
        cx.notify();
        self.create_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = create.await;
            this.update_in(cx, |this, window, cx| {
                this.creating = false;
                match result {
                    Ok(_) => {
                        this.connection
                            .update(cx, |c, cx| c.invalidate_collections(cx));
                        this.run(job, window, cx);
                    }
                    Err(e) => {
                        let detail = long_error_text(&e, &profile);
                        let text = t!("transfer.import.create_failed", detail = detail);
                        this.create_error = Some(text.to_string().into());
                        cx.notify();
                    }
                }
            })
            .ok();
        }));
    }

    fn run(&mut self, job: ImportJob, window: &mut Window, cx: &mut Context<Self>) {
        let label = self
            .source
            .as_ref()
            .and_then(|loaded| loaded.name.clone())
            .unwrap_or_else(|| t!("transfer.import.clipboard_name").to_string().into());
        let jobs = cx.global::<JobsGlobal>().0.clone();
        let connection = self.connection.clone();
        jobs.update(cx, |jobs, cx| {
            jobs.start_import(&connection, job, label, cx)
        });
        window.close_dialog(cx);
        RootView::reveal_jobs(window, cx);
    }

    fn render_source(&self, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        let shown = self.source.as_ref().map(|loaded| loaded.shown.clone());
        h_flex()
            .gap_2()
            .items_center()
            .child(field_label("transfer.import.file_label"))
            .child(
                div()
                    .id("transfer.import.source")
                    .test_support()
                    .aria_label(shown.clone().unwrap_or_default())
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .children(shown),
            )
            .child(
                Button::new("transfer.import.choose")
                    .outline()
                    .small()
                    .label(t!("transfer.import.choose").to_string())
                    .children(Kbd::binding_for_action(
                        &PickImportFile,
                        Some(IMPORT_DIALOG),
                        window,
                    ))
                    .on_click(cx.listener(|this, _, window, cx| this.choose_file(window, cx))),
            )
            .child(
                Button::new("transfer.import.paste")
                    .outline()
                    .small()
                    .label(t!("transfer.import.paste").to_string())
                    .children(Kbd::binding_for_action(
                        &PasteImport,
                        Some(IMPORT_DIALOG),
                        window,
                    ))
                    .on_click(cx.listener(|this, _, _, cx| this.paste(cx))),
            )
    }

    fn render_format(&self, cx: &Context<Self>) -> impl IntoElement {
        let formats = ButtonGroup::new("transfer.import.format").outline().small();
        let formats = FORMATS.into_iter().fold(formats, |group, (format, id)| {
            group.child(
                Button::new(SharedString::from(format!("transfer.import.format.{id}")))
                    .label(t!(format!("transfer.import.format.{id}")).to_string())
                    .selected(self.format == format)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_format(format, cx))),
            )
        });
        let delimiters = ButtonGroup::new("transfer.import.delimiter")
            .outline()
            .small();
        let delimiters = DELIMITERS
            .into_iter()
            .fold(delimiters, |group, (byte, id)| {
                group.child(
                    Button::new(SharedString::from(format!(
                        "transfer.import.delimiter.{id}"
                    )))
                    .label(t!(format!("transfer.import.delimiter.{id}")).to_string())
                    .selected(self.csv.delimiter == byte)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.csv.delimiter = byte;
                        this.refresh(cx);
                    })),
                )
            });
        h_flex()
            .gap_3()
            .items_center()
            .child(field_label("transfer.import.format"))
            .child(formats)
            .when(self.format == Format::Csv, |el| {
                el.child(field_label("transfer.import.delimiter"))
                    .child(delimiters)
                    .child(
                        Checkbox::new("transfer.import.header")
                            .checked(self.csv.header)
                            .label(t!("transfer.import.header").to_string())
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.csv.header = *checked;
                                this.refresh(cx);
                            })),
                    )
            })
    }

    fn render_options(&self, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        // Not a `ButtonGroup`: it overwrites the `disabled` of its children.
        let actions = h_flex().gap_1().children(ACTIONS.into_iter().map(|action| {
            let name = action.as_str();
            let needs_existing = matches!(action, ImportAction::Update | ImportAction::Emplace);
            Button::new(SharedString::from(format!("transfer.import.action.{name}")))
                .outline()
                .small()
                .label(name)
                .selected(self.action == action)
                .disabled(self.new.is_some() && needs_existing)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.action = action;
                    cx.notify();
                }))
        }));
        let dirties = ButtonGroup::new("transfer.import.dirty").outline().small();
        let dirties = DIRTY.into_iter().fold(dirties, |group, dirty| {
            let name = dirty.as_str();
            group.child(
                Button::new(SharedString::from(format!("transfer.import.dirty.{name}")))
                    .label(name)
                    .selected(self.dirty == dirty)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.dirty = dirty;
                        cx.notify();
                    })),
            )
        });
        let help = t!(format!(
            "transfer.import.action_help.{}",
            self.action.as_str()
        ))
        .to_string();
        let batch_error = self.batch_value(cx).is_none().then(|| {
            let text = t!(
                "transfer.import.batch_range",
                max = grouped(u64::from(MAX_BATCH))
            );
            note(
                "transfer.import.batch_error",
                text.to_string(),
                cx.theme().danger,
            )
        });
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(field_label("transfer.import.target"))
                    .child(
                        div().w(px(280.)).child(
                            Select::new(&self.target)
                                .id("transfer.import.target")
                                .small()
                                .accessibility_label(t!("transfer.import.target").to_string())
                                .placeholder(t!("transfer.import.target").to_string()),
                        ),
                    )
                    .when(self.new.is_some(), |el| {
                        el.child(
                            div().w(px(220.)).child(
                                Input::new(&self.new_name)
                                    .small()
                                    .id("transfer.import.new_name")
                                    .aria_label(t!("transfer.import.new_name").to_string()),
                            ),
                        )
                        .child(
                            Button::new("transfer.import.edit_schema")
                                .outline()
                                .small()
                                .disabled(self.new.as_ref().is_none_or(|n| n.draft.is_none()))
                                .label(t!("transfer.import.edit_schema").to_string())
                                .children(Kbd::binding_for_action(
                                    &EditImportSchema,
                                    Some(IMPORT_DIALOG),
                                    window,
                                ))
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.edit_schema(window, cx)),
                                ),
                        )
                    }),
            )
            .children(self.render_new(cx))
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(field_label("transfer.import.action"))
                    .child(actions),
            )
            .child(note(
                "transfer.import.action_help",
                help,
                cx.theme().muted_foreground,
            ))
            .when(self.new.is_some(), |el| {
                el.child(note(
                    "transfer.import.no_update_new",
                    t!("transfer.import.no_update_new").to_string(),
                    cx.theme().muted_foreground,
                ))
            })
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(field_label("transfer.import.dirty"))
                    .child(dirties),
            )
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(field_label("transfer.import.batch"))
                    .child(
                        div().w(px(120.)).child(
                            Input::new(&self.batch)
                                .small()
                                .id("transfer.import.batch")
                                .aria_label(t!("transfer.import.batch").to_string()),
                        ),
                    )
                    .children(batch_error),
            )
    }

    /// The proposal for a new collection: fields, notes, and what is wrong with it.
    fn render_new(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let new = self.new.as_ref()?;
        let (danger, muted) = (cx.theme().danger, cx.theme().muted_foreground);
        let mut lines = v_flex().gap_1();
        if let Some(error) = &new.error {
            lines = lines.child(note("transfer.infer.error", error.to_string(), danger));
        }
        if let Some(draft) = &new.draft {
            let fields = draft
                .fields
                .iter()
                .map(|f| format!("{}: {}", f.name, f.r#type.as_str()))
                .collect::<Vec<_>>()
                .join(", ");
            let sampled = t!("transfer.infer.sampled", count = grouped(new.sampled)).to_string();
            lines = lines
                .child(note("transfer.infer.fields", fields, muted))
                .child(note("transfer.infer.sampled", sampled, muted));
        }
        let notes = infer_notes(&new.notes, cx, |this, field, _, cx| {
            this.use_geopoint(field, cx);
        });
        lines = lines.children(notes);
        let errors = self.draft_errors();
        if !errors.is_empty() {
            lines = lines.child(note(
                "transfer.import.name_error",
                errors.join("\n"),
                danger,
            ));
        }
        Some(lines.into_any_element())
    }

    fn render_row(&self, ix: usize, row: &PreviewRow, cx: &App) -> AnyElement {
        let text = SharedString::from(format!("{}  {}", row.line, row.text));
        let mut issues: Vec<String> = row
            .issues
            .iter()
            .map(|i| {
                let message = t!(i.key, detail = i.detail);
                format!("{}: {message}", i.path)
            })
            .collect();
        match &row.error {
            Some(PreviewError::Json(detail)) => issues.push(detail.clone()),
            Some(PreviewError::InvalidUtf8) => {
                issues.push(t!("transfer.import.rejected_utf8").to_string());
            }
            Some(PreviewError::TooLarge(bytes)) => issues.push(
                t!(
                    "transfer.import.rejected_large",
                    size = size(*bytes),
                    limit = size(MAX_DOC_BYTES as u64)
                )
                .to_string(),
            ),
            None => {}
        }
        v_flex()
            .child(
                div()
                    .id(SharedString::from(format!(
                        "transfer.import.preview.row.{ix}"
                    )))
                    .test_support()
                    .aria_label(text.clone())
                    .truncate()
                    .text_xs()
                    .child(text),
            )
            .when(!issues.is_empty(), |el| {
                let id = format!("transfer.import.preview.issue.{ix}");
                el.child(note_owned(id, issues.join("\n"), cx.theme().warning))
            })
            .into_any_element()
    }

    fn render_preview(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        match self.preview.as_ref()? {
            Err(e) => Some(
                note(
                    "transfer.import.preview.error",
                    read_error_text(e),
                    cx.theme().danger,
                )
                .into_any_element(),
            ),
            Ok(preview) => {
                let docs = grouped(preview.estimated_docs);
                let title = if preview.complete {
                    t!("transfer.import.preview_exact", docs = docs)
                } else {
                    let count = preview.rows.len();
                    t!("transfer.import.preview", count = count, docs = docs)
                };
                Some(
                    v_flex()
                        .gap_1()
                        .child(note(
                            "transfer.import.preview.title",
                            title.to_string(),
                            cx.theme().muted_foreground,
                        ))
                        .child(
                            v_flex()
                                .id("transfer.import.preview")
                                .h(px(PREVIEW_HEIGHT))
                                .overflow_y_scroll()
                                .children(
                                    preview
                                        .rows
                                        .iter()
                                        .enumerate()
                                        .map(|(ix, row)| self.render_row(ix, row, cx)),
                                ),
                        )
                        .into_any_element(),
                )
            }
        }
    }
}

/// The name of a row of options.
fn field_label(key: &'static str) -> impl IntoElement {
    div()
        .flex_none()
        .w(px(90.))
        .text_sm()
        .child(t!(key).to_string())
}

impl Render for DocumentImportDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let locked = self.connection.read(cx).writes_locked();
        let can_start = self.job(cx).is_some();
        let source = self.render_source(window, cx);
        let format = self.render_format(cx);
        let options = self.render_options(window, cx);
        let preview = self.render_preview(cx);
        let list_error = self.connection.read(cx).collection_cache.error.clone();
        let theme = cx.theme();
        let (danger, muted, warning) = (theme.danger, theme.muted_foreground, theme.warning);
        v_flex()
            .key_context(IMPORT_DIALOG)
            .track_focus(&self.focus_handle)
            .gap_2()
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                if let Some(path) = paths.paths().first() {
                    this.read_file(path.clone(), cx);
                }
            }))
            .on_action(cx.listener(|this, _: &PickImportFile, window, cx| {
                this.choose_file(window, cx);
            }))
            .on_action(cx.listener(|this, _: &PasteImport, _, cx| this.paste(cx)))
            .on_action(cx.listener(|this, _: &EditImportSchema, window, cx| {
                this.edit_schema(window, cx);
            }))
            .on_action(cx.listener(|this, _: &StartTransfer, window, cx| this.start(window, cx)))
            .child(source)
            .children(
                self.source_error
                    .clone()
                    .map(|e| note("transfer.import.source_error", e.to_string(), danger)),
            )
            .when(self.file_changed, |el| {
                let text = t!("transfer.import.file_changed").to_string();
                el.child(note("transfer.import.file_changed", text, warning))
            })
            .child(format)
            .children(list_error.map(|detail| {
                let text = t!("transfer.import.list_error", detail = detail).to_string();
                note("transfer.import.list_error", text, danger)
            }))
            .child(options)
            .children(preview)
            .children(
                self.create_error
                    .clone()
                    .map(|e| note("transfer.import.create_error", e.to_string(), danger)),
            )
            .when(locked, |el| {
                el.child(note(
                    "transfer.import.locked",
                    t!("transfer.import.locked").to_string(),
                    muted,
                ))
            })
            .child(note(
                "transfer.import.note",
                t!("transfer.import.cancel_note").to_string(),
                muted,
            ))
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("transfer.import.cancel")
                            .label(t!("common.cancel").to_string())
                            .children(Kbd::binding_for_action(&Cancel, Some("Dialog"), window))
                            .disabled(self.creating)
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("transfer.import.start")
                            .primary()
                            .label(t!("transfer.import.start").to_string())
                            .when(locked, |b| {
                                b.child(env_badge::lock("transfer.import.start", false, cx))
                            })
                            .children(Kbd::binding_for_action(
                                &StartTransfer,
                                Some(IMPORT_DIALOG),
                                window,
                            ))
                            .loading(self.creating)
                            .disabled(!can_start || self.creating)
                            .on_click(cx.listener(|this, _, window, cx| this.start(window, cx))),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_size_takes_the_range() {
        assert_eq!(batch_size("1"), Some(1));
        assert_eq!(batch_size(" 1000 "), Some(1000));
        assert_eq!(batch_size("100000"), Some(MAX_BATCH));
        for bad in ["0", "100001", "-1", "1.5", "", "x"] {
            assert_eq!(batch_size(bad), None, "{bad}");
        }
    }
}
