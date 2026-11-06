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
use tessera_client::{Collection, DirtyValues, ImportAction, ImportParams};
use tessera_core::transfer::import::ImportJob;
use tessera_core::transfer::preview::{Preview, PreviewError, PreviewRow};
use tessera_core::transfer::source::{CsvOptions, Format, ReadError, ReadOptions, Source, sniff};
use tessera_core::transfer::{CLIPBOARD_LIMIT, DEFAULT_BATCH, MAX_BATCH, MAX_DOC_BYTES};

use super::super::env_badge;
use super::super::overview::grouped;
use super::super::request_detail::size;
use super::super::root::RootView;
use crate::actions::{IMPORT_DIALOG, PasteImport, PickImportFile, StartTransfer};
use crate::connection::ConnectionState;
use crate::jobs::JobsGlobal;
use crate::runtime::Tokio;
use crate::state::AppState;
use crate::transfer::{TransferBackendGlobal, read_error_text};

const WIDTH: f32 = 900.;
/// A change settles for this long before the preview reruns.
const PREVIEW_DELAY: Duration = Duration::from_millis(150);
/// Enough of a file to tell its format.
const HEAD_BYTES: usize = 4096;
const PREVIEW_HEIGHT: f32 = 180.;

const FORMATS: [(Format, &str, &str); 3] = [
    (Format::Jsonl, "jsonl", "JSONL"),
    (Format::JsonArray, "json", "JSON"),
    (Format::Csv, "csv", "CSV"),
];
const DELIMITERS: [(u8, &str, &str); 4] = [
    (b',', "comma", ","),
    (b';', "semicolon", ";"),
    (b'\t', "tab", "tab"),
    (b'|', "pipe", "|"),
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

/// A collection the select offers.
#[derive(Clone, PartialEq)]
struct TargetItem(String);

impl SearchableListItem for TargetItem {
    type Value = String;

    fn title(&self) -> SharedString {
        self.0.clone().into()
    }

    fn value(&self) -> &String {
        &self.0
    }
}

/// What will be read.
#[derive(Clone)]
struct Loaded {
    source: Source,
    /// The file name; `None` for the clipboard.
    name: Option<SharedString>,
    /// The line above the options.
    shown: SharedString,
}

/// Why a path cannot be a source.
enum Inspect {
    NotAFile,
    Io(String),
}

/// The size and first bytes of `path`.
fn inspect(path: &Path) -> Result<(u64, Vec<u8>), Inspect> {
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

/// `n` documents per batch, if `text` is a size the import takes.
fn batch_size(text: &str) -> Option<u32> {
    text.trim()
        .parse()
        .ok()
        .filter(|n| (1..=MAX_BATCH).contains(n))
}

/// The dialog's state. Closing it stops a preview; a started job is not its own.
pub struct DocumentImportDialog {
    connection: Entity<ConnectionState>,
    focus_handle: FocusHandle,
    source: Option<Loaded>,
    source_error: Option<SharedString>,
    source_task: Option<Task<()>>,
    format: Format,
    csv: CsvOptions,
    target: Entity<SelectState<Vec<TargetItem>>>,
    /// The names `target` holds.
    offered: Vec<String>,
    /// The chosen collection.
    collection: Option<String>,
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
        _state: Entity<AppState>,
        connection: Entity<ConnectionState>,
        target: Option<String>,
        source: Option<Source>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let profile = connection.read(cx).profile.clone();
        let view = cx.new(|cx| Self::new(connection, target, source, window, cx));
        let focus = view.read(cx).focus_handle.clone();
        let first_build = Cell::new(true);
        let dialog_view = view.clone();
        window.open_dialog(cx, move |dialog, window, cx| {
            if first_build.replace(false) {
                focus.focus(window, cx);
            }
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
                .child(dialog_view.clone())
        });
        view
    }

    fn new(
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
        let subscriptions = vec![
            cx.observe_in(&connection, window, |this, _, window, cx| {
                this.collections_changed(window, cx);
            }),
            cx.subscribe_in(
                &select,
                window,
                |this, _, event: &SelectEvent<Vec<TargetItem>>, _, cx| {
                    if let SelectEvent::Confirm(Some(name)) = event {
                        this.pick_target(name.clone(), cx);
                    }
                },
            ),
            cx.subscribe(&batch, |_, _, _: &InputEvent, cx| cx.notify()),
        ];
        let mut this = Self {
            connection,
            focus_handle: cx.focus_handle(),
            source: None,
            source_error: None,
            source_task: None,
            format: Format::Jsonl,
            csv: CsvOptions::default(),
            target: select,
            offered: Vec::new(),
            collection: target,
            action: ImportAction::Create,
            dirty: DirtyValues::CoerceOrReject,
            batch,
            preview: None,
            preview_task: None,
            _subscriptions: subscriptions,
        };
        let loaded = this
            .connection
            .read(cx)
            .collection_cache
            .collections
            .is_some();
        if loaded {
            this.collections_changed(window, cx);
        } else {
            this.connection
                .update(cx, |c, cx| c.load_collection_list(cx));
        }
        match source {
            Some(Source::File(path)) => this.read_file(path, cx),
            Some(Source::Bytes(bytes)) => this.read_bytes(bytes, cx),
            None => {}
        }
        this
    }

    /// The names of the connection's collections.
    fn names(&self, cx: &App) -> Vec<String> {
        let cache = &self.connection.read(cx).collection_cache;
        let all = cache.collections.iter().flatten();
        all.map(|c| c.name.clone()).collect()
    }

    /// Fills the select from the list, which may arrive after the dialog opened.
    fn collections_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let names = self.names(cx);
        // The connection also notifies for changes that leave the list as it was.
        if names == self.offered {
            cx.notify();
            return;
        }
        self.offered = names.clone();
        // A collection that went away is no target; the list keeps it out of the job too.
        self.collection = self.collection.take().filter(|name| names.contains(name));
        let items: Vec<_> = names.iter().cloned().map(TargetItem).collect();
        let row = self
            .collection
            .as_ref()
            .and_then(|name| names.iter().position(|n| n == name));
        self.target.update(cx, |select, cx| {
            select.set_items(items, window, cx);
            let index = row.map(|row| IndexPath::default().row(row));
            select.set_selected_index(index, window, cx);
        });
        self.schedule_preview(cx);
    }

    /// Chooses the collection to import into.
    pub fn pick_target(&mut self, name: String, cx: &mut Context<Self>) {
        self.collection = Some(name);
        self.schedule_preview(cx);
    }

    /// The schema of the chosen collection.
    fn schema(&self, cx: &App) -> Option<Collection> {
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
        let shown = path.clone();
        let read = tokio.spawn_blocking(move || inspect(&shown));
        self.source_task = Some(cx.spawn(async move |this, cx| {
            let read = read.await;
            this.update(cx, |this, cx| match read {
                Ok(Ok((bytes, head))) => {
                    let name: Option<SharedString> = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned().into());
                    let (format, csv) = sniff(Some(&path), &head);
                    (this.format, this.csv) = (format, csv);
                    this.source = Some(Loaded {
                        shown: t!(
                            "transfer.import.file",
                            name = name.clone().unwrap_or_default(),
                            size = size(bytes)
                        )
                        .to_string()
                        .into(),
                        source: Source::File(path),
                        name,
                    });
                    this.schedule_preview(cx);
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
        self.source_error = None;
        self.source = Some(Loaded {
            shown: t!("transfer.import.clipboard", size = size(bytes.len() as u64))
                .to_string()
                .into(),
            source: Source::Bytes(bytes),
            name: None,
        });
        self.schedule_preview(cx);
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
        self.schedule_preview(cx);
    }

    fn batch_value(&self, cx: &App) -> Option<u32> {
        batch_size(&self.batch.read(cx).value())
    }

    /// What Start would run, when everything is valid.
    fn job(&self, cx: &App) -> Option<ImportJob> {
        let source = self.source.as_ref()?.source.clone();
        let collection = self.collection.clone()?;
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

    /// Validates, asks to unlock a read-only connection, then runs the job.
    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(job) = self.job(cx) else {
            return;
        };
        let dialog = cx.entity();
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                dialog.update(cx, |d, cx| d.run(job, window, cx));
            });
        });
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
        let formats = FORMATS
            .into_iter()
            .fold(formats, |group, (format, id, text)| {
                group.child(
                    Button::new(SharedString::from(format!("transfer.import.format.{id}")))
                        .label(text)
                        .selected(self.format == format)
                        .on_click(cx.listener(move |this, _, _, cx| this.set_format(format, cx))),
                )
            });
        let delimiters = ButtonGroup::new("transfer.import.delimiter")
            .outline()
            .small();
        let delimiters = DELIMITERS
            .into_iter()
            .fold(delimiters, |group, (byte, id, text)| {
                group.child(
                    Button::new(SharedString::from(format!(
                        "transfer.import.delimiter.{id}"
                    )))
                    .label(text)
                    .selected(self.csv.delimiter == byte)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.csv.delimiter = byte;
                        this.schedule_preview(cx);
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
                                this.schedule_preview(cx);
                            })),
                    )
            })
    }

    fn render_options(&self, cx: &Context<Self>) -> impl IntoElement {
        let actions = ButtonGroup::new("transfer.import.action").outline().small();
        let actions = ACTIONS.into_iter().fold(actions, |group, action| {
            let name = action.as_str();
            group.child(
                Button::new(SharedString::from(format!("transfer.import.action.{name}")))
                    .label(name)
                    .selected(self.action == action)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.action = action;
                        cx.notify();
                    })),
            )
        });
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
                    ),
            )
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

fn note(id: &'static str, text: String, color: gpui_kit::Hsla) -> impl IntoElement {
    note_owned(id.to_string(), text, color)
}

fn note_owned(id: String, text: String, color: gpui_kit::Hsla) -> impl IntoElement {
    let text = SharedString::from(text);
    div()
        .id(SharedString::from(id))
        .test_support()
        .aria_label(text.clone())
        .text_xs()
        .text_color(color)
        .child(text)
}

impl Render for DocumentImportDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let locked = self.connection.read(cx).writes_locked();
        let can_start = self.job(cx).is_some();
        let source = self.render_source(window, cx);
        let format = self.render_format(cx);
        let options = self.render_options(cx);
        let preview = self.render_preview(cx);
        let theme = cx.theme();
        let (danger, muted) = (theme.danger, theme.muted_foreground);
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
            .on_action(cx.listener(|this, _: &StartTransfer, window, cx| this.start(window, cx)))
            .child(source)
            .children(
                self.source_error
                    .clone()
                    .map(|e| note("transfer.import.source_error", e.to_string(), danger)),
            )
            .child(format)
            .child(options)
            .children(preview)
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
                            .disabled(!can_start)
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
