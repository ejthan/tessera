// SPDX-License-Identifier: GPL-3.0-or-later

//! The jobs tab of the bottom dock: every import and export of the session, the selected
//! job's numbers and what it could not store.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dock::{BasePanel, Panel, PanelEvent};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::progress::Progress;
use gpui_kit::component::table::{Column, DataTable, TableDelegate, TableEvent, TableState};
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, h_flex, resizable_panel, v_flex, v_resizable,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    ParentElement as _, Render, SharedString, Stateful, Styled as _, Subscription, Task, Window,
    div, prelude::*, px,
};
use rust_i18n::t;
use tessera_core::transfer::report::Report;
use tessera_core::transfer::source::Source;
use tessera_widgets::confirm;

use super::super::env_badge;
use super::super::overview::grouped;
use super::super::request_detail::size;
use super::import_dialog::DocumentImportDialog;
use crate::actions::{
    CancelJob, DismissJob, JOBS, SaveFailedLines, SaveReport, StopWaiting, gated,
};
use crate::jobs::{Job, JobId, JobKind, JobOutcome, JobStatus, Jobs, JobsGlobal, Live};
use crate::state::AppState;
use crate::transfer::TransferBackendGlobal;

/// Initial height of the detail under the job table.
const DETAIL_HEIGHT: f32 = 130.;
/// Width of the buttons and sentences beside the report.
const NOTES_WIDTH: f32 = 420.;

/// The columns of the job table, in order.
#[derive(Clone, Copy, PartialEq)]
enum Col {
    Kind,
    Connection,
    Target,
    Progress,
    Text,
}

const COLUMNS: [Col; 5] = [
    Col::Kind,
    Col::Connection,
    Col::Target,
    Col::Progress,
    Col::Text,
];

impl Col {
    fn key(self) -> &'static str {
        match self {
            Col::Kind => "kind",
            Col::Connection => "connection",
            Col::Target => "target",
            Col::Progress => "progress",
            Col::Text => "text",
        }
    }

    fn width(self) -> f32 {
        match self {
            Col::Kind => 90.,
            Col::Connection => 150.,
            Col::Target => 220.,
            Col::Progress => 110.,
            Col::Text => 420.,
        }
    }
}

/// `m:ss`, or `h:mm:ss` from an hour.
fn eta_text(eta: Duration) -> String {
    let seconds = eta.as_secs();
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// What a job's row says about its numbers.
fn live_text(job: &Job) -> String {
    match &job.live {
        Live::Import(p) if job.is_running() => {
            let eta = p
                .eta()
                .map_or_else(|| t!("figure.missing").to_string(), eta_text);
            t!(
                "jobs.import_live",
                docs = grouped(p.imported + p.failed),
                rate = grouped(p.rate().round() as u64),
                eta = eta,
                errors = grouped(p.failed)
            )
            .to_string()
        }
        Live::Import(p) => t!(
            "jobs.done_import",
            imported = grouped(p.imported),
            failed = grouped(p.failed),
            unconfirmed = grouped(p.unconfirmed)
        )
        .to_string(),
        Live::Export(p) => t!(
            "jobs.export_live",
            docs = grouped(p.docs),
            total = p
                .expected
                .map_or_else(|| t!("figure.missing").to_string(), grouped),
            bytes = size(p.bytes)
        )
        .to_string(),
    }
}

/// The word for what a job does.
fn kind_text(kind: JobKind) -> String {
    match kind {
        JobKind::Import => t!("jobs.import_row").to_string(),
        JobKind::Export => t!("jobs.export_row").to_string(),
    }
}

/// The word for where a job stands.
fn status_text(status: &JobStatus) -> String {
    match status {
        JobStatus::Running => t!("jobs.status.running"),
        JobStatus::Stopping => t!("jobs.status.stopping"),
        JobStatus::Finished => t!("jobs.status.finished"),
        JobStatus::Stopped => t!("jobs.status.stopped"),
        JobStatus::Failed(_) => t!("jobs.status.failed"),
    }
    .to_string()
}

/// Share of a running job that is done, when its total is known.
fn fraction(job: &Job) -> Option<f32> {
    if !job.is_running() {
        return None;
    }
    match &job.live {
        Live::Import(p) => p.fraction(),
        Live::Export(p) => {
            let total = p.expected.filter(|total| *total > 0)?;
            Some((p.docs as f32 / total as f32).clamp(0., 1.))
        }
    }
}

/// The job table reads [`Jobs`] on each render, so live cells never go stale.
struct JobRows {
    jobs: Entity<Jobs>,
}

impl JobRows {
    fn with_job<R>(&self, ix: usize, cx: &App, f: impl FnOnce(&Job) -> R) -> Option<R> {
        self.jobs.read(cx).jobs().get(ix).map(f)
    }

    fn text(&self, row_ix: usize, col: Col, cx: &App) -> String {
        self.with_job(row_ix, cx, |job| match col {
            Col::Kind => format!("{} · {}", kind_text(job.kind), status_text(&job.status)),
            Col::Connection => format!(
                "{} · {}",
                job.connection_name,
                env_badge::label(&job.environment)
            ),
            Col::Target => format!("{} · {}", job.collection, job.label),
            Col::Progress => fraction(job).map_or_else(String::new, percent),
            Col::Text => live_text(job),
        })
        .unwrap_or_default()
    }
}

fn percent(fraction: f32) -> String {
    t!("jobs.percent", n = format!("{:.0}", fraction * 100.)).to_string()
}

impl TableDelegate for JobRows {
    fn columns_count(&self, _: &App) -> usize {
        COLUMNS.len()
    }

    fn rows_count(&self, cx: &App) -> usize {
        self.jobs.read(cx).jobs().len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        let col = COLUMNS[col_ix];
        Column::new(col.key(), t!(format!("jobs.col.{}", col.key())).to_string())
            .width(px(col.width()))
    }

    fn render_tr(
        &mut self,
        row_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> Stateful<gpui_kit::Div> {
        let Some(id) = self.with_job(row_ix, cx, |job| job.id) else {
            return div().id(("jobs.filler", row_ix));
        };
        let label = COLUMNS
            .iter()
            .map(|col| self.text(row_ix, *col, cx))
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        div()
            .id(SharedString::from(format!("jobs.row.{}", id.0)))
            .aria_label(label)
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let col = COLUMNS[col_ix];
        let text = SharedString::from(self.text(row_ix, col, cx));
        let cell = div().truncate();
        let Some((id, environment, name, share, failed)) = self.with_job(row_ix, cx, |job| {
            (
                job.id,
                job.environment.clone(),
                job.connection_name.clone(),
                fraction(job),
                matches!(job.status, JobStatus::Failed(_)),
            )
        }) else {
            return cell.into_any_element();
        };
        match col {
            Col::Connection => h_flex()
                .gap_1()
                .items_center()
                .child(env_badge::render(&environment, cx))
                .child(div().truncate().child(name))
                .into_any_element(),
            Col::Progress => match share {
                Some(share) => div()
                    .id(SharedString::from(format!("jobs.progress.{}", id.0)))
                    .test_support()
                    .aria_label(text.clone())
                    .w_full()
                    .child(
                        Progress::new(("jobs.bar", id.0))
                            .value(share * 100.)
                            .accessibility_label(text),
                    )
                    .into_any_element(),
                None => cell.into_any_element(),
            },
            Col::Text => div()
                .id(SharedString::from(format!("jobs.text.{}", id.0)))
                .test_support()
                .aria_label(text.clone())
                .truncate()
                .child(text)
                .into_any_element(),
            Col::Kind => cell
                .when(failed, |el| el.text_color(cx.theme().danger))
                .child(text)
                .into_any_element(),
            Col::Target => cell.child(text).into_any_element(),
        }
    }

    fn cell_label(&self, row_ix: usize, col_ix: usize, cx: &App) -> Option<SharedString> {
        Some(self.text(row_ix, COLUMNS[col_ix], cx).into())
    }

    fn cell_text(&self, row_ix: usize, col_ix: usize, cx: &App) -> String {
        self.text(row_ix, COLUMNS[col_ix], cx)
    }
}

/// The columns of the report table, with the keys of their titles and their widths.
const REPORT_COLUMNS: [(&str, f32); 4] = [
    ("line", 70.),
    ("code", 60.),
    ("error", 360.),
    ("document", 460.),
];

/// The failed rows a report keeps in memory.
#[derive(Default)]
struct ReportRows {
    report: Option<Arc<Report>>,
}

impl ReportRows {
    fn text(&self, row_ix: usize, col_ix: usize) -> String {
        let row = self.report.as_ref().and_then(|r| r.rows().get(row_ix));
        let Some(row) = row else {
            return String::new();
        };
        match col_ix {
            0 => row.line.to_string(),
            1 => row.code.map(|c| c.to_string()).unwrap_or_default(),
            2 => row.error.clone(),
            _ => row.raw.clone(),
        }
    }
}

impl TableDelegate for ReportRows {
    fn columns_count(&self, _: &App) -> usize {
        REPORT_COLUMNS.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.report.as_ref().map_or(0, |r| r.rows().len())
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        let (key, width) = REPORT_COLUMNS[col_ix];
        Column::new(key, t!(format!("jobs.report.{key}")).to_string()).width(px(width))
    }

    fn render_tr(
        &mut self,
        row_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> Stateful<gpui_kit::Div> {
        let label = (0..REPORT_COLUMNS.len())
            .map(|col| self.text(row_ix, col))
            .collect::<Vec<_>>()
            .join(" · ");
        div()
            .id(SharedString::from(format!("jobs.report.row.{row_ix}")))
            .aria_label(label)
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div().truncate().child(self.text(row_ix, col_ix))
    }

    fn cell_label(&self, row_ix: usize, col_ix: usize, _: &App) -> Option<SharedString> {
        Some(self.text(row_ix, col_ix).into())
    }

    fn cell_text(&self, row_ix: usize, col_ix: usize, _: &App) -> String {
        self.text(row_ix, col_ix)
    }
}

/// The result of the last save.
struct Saved {
    job: JobId,
    text: Result<SharedString, SharedString>,
}

/// The jobs tab.
pub struct JobsPanel {
    state: Entity<AppState>,
    jobs: Entity<Jobs>,
    table: Entity<TableState<JobRows>>,
    report: Entity<TableState<ReportRows>>,
    /// The job the user picked; without one the newest shows.
    picked: Option<JobId>,
    saved: Option<Saved>,
    /// The last saved failed lines: the job and the file.
    failed_file: Option<(JobId, PathBuf)>,
    save_task: Option<Task<()>>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl JobsPanel {
    /// The panel over the app's jobs.
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let jobs = cx.global::<JobsGlobal>().0.clone();
        let rows = JobRows { jobs: jobs.clone() };
        let table = cx.new(|cx| {
            TableState::new(rows, window, cx)
                .sortable(false)
                .col_movable(false)
                .col_selectable(false)
        });
        let report = cx.new(|cx| {
            TableState::new(ReportRows::default(), window, cx)
                .sortable(false)
                .col_movable(false)
                .col_selectable(false)
        });
        let subscriptions = vec![
            cx.observe(&jobs, |this, _, cx| this.refresh(cx)),
            cx.subscribe(&table, |this, _, event: &TableEvent, cx| {
                if let TableEvent::SelectRow(ix) = event {
                    this.pick(*ix, cx);
                }
            }),
        ];
        Self {
            state,
            jobs,
            table,
            report,
            picked: None,
            saved: None,
            failed_file: None,
            save_task: None,
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    /// The job the panel shows: the picked one, or the newest.
    fn current<'a>(&self, cx: &'a App) -> Option<&'a Job> {
        let jobs = self.jobs.read(cx).jobs();
        self.picked
            .and_then(|id| jobs.iter().find(|job| job.id == id))
            .or(jobs.last())
    }

    fn pick(&mut self, ix: usize, cx: &mut Context<Self>) {
        let picked = self.jobs.read(cx).jobs().get(ix).map(|job| job.id);
        // The table also reports the selection `refresh` made.
        if picked != self.current(cx).map(|job| job.id) {
            self.picked = picked;
            self.refresh(cx);
        }
    }

    /// Brings both tables up to the jobs. Runs at most at the rate of the job updates.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let current = self.current(cx);
        let row = current.and_then(|job| {
            let id = job.id;
            self.jobs.read(cx).jobs().iter().position(|j| j.id == id)
        });
        let report = current
            .and_then(Job::report)
            .filter(|report| report.total() > 0)
            .cloned();
        self.table.update(cx, |table, cx| {
            match row {
                Some(row) if table.selected_row() != Some(row) => table.set_selected_row(row, cx),
                None if table.selected_row().is_some() => table.clear_selection(cx),
                _ => {}
            }
            cx.notify();
        });
        self.report.update(cx, |table, cx| {
            let held = &table.delegate().report;
            let same = match (held, &report) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            };
            if !same {
                table.delegate_mut().report = report;
                cx.notify();
            }
        });
        cx.notify();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.current(cx).map(|job| job.id) {
            self.jobs.update(cx, |jobs, cx| jobs.stop(id, cx));
        }
    }

    fn stop_waiting(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.current(cx).map(|job| job.id) {
            self.jobs.update(cx, |jobs, cx| jobs.abort(id, cx));
        }
    }

    /// Removes the shown job, asking first when that loses an unsaved report.
    fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(job) = self.current(cx).filter(|job| !job.is_running()) else {
            return;
        };
        let id = job.id;
        let unsaved = !job.report_saved && job.report().is_some_and(|r| r.total() > 0);
        let jobs = self.jobs.clone();
        if unsaved {
            confirm::open(
                t!("jobs.dismiss_title").to_string(),
                t!("jobs.dismiss_body").to_string(),
                t!("jobs.dismiss").to_string(),
                move |_, cx| jobs.update(cx, |jobs, cx| jobs.dismiss(id, cx)),
                window,
                cx,
            );
        } else {
            jobs.update(cx, |jobs, cx| jobs.dismiss(id, cx));
        }
    }

    /// Asks where to, then saves the report of the shown job, or with `failed_only` its
    /// failed lines.
    fn save(&mut self, failed_only: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((id, collection, report)) = self.current(cx).and_then(|job| {
            let report = job.report().filter(|r| r.total() > 0)?.clone();
            Some((job.id, job.collection.clone(), report))
        }) else {
            return;
        };
        let suffix = if failed_only { "failed" } else { "report" };
        let dir = std::env::home_dir().unwrap_or_default();
        let picked = cx.prompt_for_new_path(&dir, Some(&format!("{collection}.{suffix}.jsonl")));
        self.saved = None;
        cx.notify();
        self.save_task = Some(cx.spawn_in(window, async move |this, cx| {
            let path: PathBuf = match picked.await {
                Ok(Ok(Some(path))) => path,
                // Cancelled.
                Ok(Ok(None)) | Err(_) => return,
                Ok(Err(e)) => {
                    let detail = e.to_string();
                    let text = t!("connect.error.picker", detail = detail).to_string();
                    this.update(cx, |this, cx| {
                        this.saved = Some(Saved {
                            job: id,
                            text: Err(text.into()),
                        });
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            let Ok(save) = this.update(cx, |_, cx| {
                let backend = cx.global::<TransferBackendGlobal>().0.clone();
                backend.save_report(report, path.clone(), failed_only)
            }) else {
                return;
            };
            let result = save.await;
            this.update(cx, |this, cx| {
                if result.is_ok() {
                    this.jobs
                        .update(cx, |jobs, cx| jobs.set_report_saved(id, cx));
                    if failed_only {
                        this.failed_file = Some((id, path.clone()));
                    }
                }
                let text = match result {
                    Ok(count) => Ok(t!(
                        "jobs.saved",
                        count = grouped(count),
                        path = path.display().to_string()
                    )
                    .to_string()
                    .into()),
                    Err(detail) => Err(t!("jobs.save_failed_text", detail = detail)
                        .to_string()
                        .into()),
                };
                this.saved = Some(Saved { job: id, text });
                cx.notify();
            })
            .ok();
        }));
    }

    /// Opens the import dialog on the saved failed lines of the shown job.
    fn import_saved(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(job) = self.current(cx) else {
            return;
        };
        let (profile, collection) = (job.connection, job.collection.clone());
        let Some(path) = self.saved_file(job) else {
            return;
        };
        if let Some(connection) = self.state.read(cx).connection(profile, cx) {
            let source = Some(Source::File(path));
            let state = self.state.clone();
            DocumentImportDialog::open(state, connection, Some(collection), source, window, cx);
        }
    }

    /// The failed lines saved from `job`.
    fn saved_file(&self, job: &Job) -> Option<PathBuf> {
        let (id, path) = self.failed_file.as_ref()?;
        (*id == job.id).then(|| path.clone())
    }

    /// The buttons of the shown job.
    fn render_actions(&self, job: &Job, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        let has_report = job.report().is_some_and(|r| r.total() > 0);
        h_flex()
            .flex_none()
            .flex_wrap()
            .gap_2()
            .px_2()
            .py_1()
            .items_center()
            .when(job.status == JobStatus::Running, |el| {
                el.child(
                    Button::new("jobs.cancel")
                        .ghost()
                        .xsmall()
                        .label(t!("jobs.cancel").to_string())
                        .children(Kbd::binding_for_action(&CancelJob, Some(JOBS), window))
                        .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                )
            })
            .when(job.status == JobStatus::Stopping, |el| {
                el.child(
                    Button::new("jobs.stop_waiting")
                        .ghost()
                        .xsmall()
                        .label(t!("jobs.stop_waiting").to_string())
                        .children(Kbd::binding_for_action(&StopWaiting, Some(JOBS), window))
                        .on_click(cx.listener(|this, _, _, cx| this.stop_waiting(cx))),
                )
            })
            .when(has_report, |el| {
                el.child(
                    Button::new("jobs.save_report")
                        .ghost()
                        .xsmall()
                        .label(t!("jobs.save_report").to_string())
                        .children(Kbd::binding_for_action(&SaveReport, Some(JOBS), window))
                        .on_click(cx.listener(|this, _, window, cx| this.save(false, window, cx))),
                )
                .child(
                    Button::new("jobs.save_failed")
                        .ghost()
                        .xsmall()
                        .label(t!("jobs.save_failed").to_string())
                        .children(Kbd::binding_for_action(
                            &SaveFailedLines,
                            Some(JOBS),
                            window,
                        ))
                        .on_click(cx.listener(|this, _, window, cx| this.save(true, window, cx))),
                )
            })
            .when(self.saved_file(job).is_some(), |el| {
                el.child(
                    Button::new("jobs.import_saved")
                        .ghost()
                        .xsmall()
                        .label(t!("jobs.import_saved").to_string())
                        .on_click(cx.listener(|this, _, window, cx| this.import_saved(window, cx))),
                )
            })
            .when(!job.is_running(), |el| {
                el.child(
                    Button::new("jobs.dismiss")
                        .ghost()
                        .xsmall()
                        .label(t!("jobs.dismiss").to_string())
                        .children(Kbd::binding_for_action(&DismissJob, Some(JOBS), window))
                        .on_click(cx.listener(|this, _, window, cx| this.dismiss(window, cx))),
                )
            })
    }

    /// The sentences about the shown job, all in full.
    fn render_notes(&self, job: &Job, cx: &App) -> Vec<gpui_kit::AnyElement> {
        let note = |id: SharedString, text: String, danger: bool| {
            let text = SharedString::from(text);
            div()
                .id(id)
                .test_support()
                .aria_label(text.clone())
                .px_2()
                .text_xs()
                .when(danger, |el| el.text_color(cx.theme().danger))
                .child(text)
                .into_any_element()
        };
        let mut notes = Vec::new();
        let failure = match &job.status {
            JobStatus::Failed(text) => Some(text.to_string()),
            _ => None,
        };
        let first_error = match &job.live {
            Live::Import(p) => p.first_error.as_deref().map(str::to_string),
            Live::Export(_) => None,
        };
        let errors = failure.into_iter().chain(first_error).collect::<Vec<_>>();
        if !errors.is_empty() {
            let id = format!("jobs.error.{}", job.id.0).into();
            notes.push(note(id, errors.join("\n"), true));
        }
        if let Some(JobOutcome::Import(outcome)) = &job.outcome {
            if let Some(line) = outcome.stopped_at_line {
                let text = t!("jobs.stopped_at", line = grouped(line)).to_string();
                notes.push(note("jobs.stopped_at".into(), text, false));
            }
            let finished = job.status == JobStatus::Finished;
            match &outcome.count {
                Some(count) if !count.ok => {
                    let grown = count.after.saturating_sub(count.before);
                    let imported = outcome.progress.imported;
                    let text = t!(
                        "jobs.count_warning",
                        grown = grouped(grown),
                        imported = grouped(imported)
                    )
                    .to_string();
                    notes.push(note("jobs.count_warning".into(), text, true));
                }
                None if finished => {
                    let text = t!("jobs.count_unchecked").to_string();
                    notes.push(note("jobs.count_unchecked".into(), text, false));
                }
                _ => {}
            }
        }
        if job.kind == JobKind::Import {
            let text = t!("jobs.stay_note").to_string();
            notes.push(note("jobs.stay_note".into(), text, false));
        }
        if let Some(saved) = self.saved.as_ref().filter(|s| s.job == job.id) {
            let (text, danger) = match &saved.text {
                Ok(text) => (text, false),
                Err(text) => (text, true),
            };
            notes.push(note("jobs.saved".into(), text.to_string(), danger));
        }
        if let Some(report) = job.report() {
            let (shown, total) = (report.rows().len() as u64, report.total());
            if shown < total {
                let text = t!(
                    "jobs.report_more",
                    shown = grouped(shown),
                    total = grouped(total)
                )
                .to_string();
                notes.push(note("jobs.report_more".into(), text, false));
            }
        }
        notes
    }
}

impl EventEmitter<PanelEvent> for JobsPanel {}

impl Focusable for JobsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for JobsPanel {
    fn panel_name(&self) -> &'static str {
        "Jobs"
    }

    // ⇧⌘J hides it; closing would leave nothing to toggle.
    fn closable(&self, _: &App) -> bool {
        false
    }
}

impl Panel for JobsPanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        t!("jobs.title").to_string()
    }
}

impl Render for JobsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cx: &Context<Self> = cx;
        let body = self.current(cx).map(|job| {
            let notes = self.render_notes(job, cx);
            let actions = self.render_actions(job, window, cx);
            let has_report = job.report().is_some_and(|r| r.total() > 0);
            h_flex()
                .size_full()
                .items_start()
                .child(
                    v_flex()
                        .id("jobs.detail")
                        .h_full()
                        .when(has_report, |el| el.flex_none().w(px(NOTES_WIDTH)))
                        .when(!has_report, |el| el.w_full())
                        .overflow_y_scroll()
                        .child(actions)
                        .children(notes),
                )
                .when(has_report, |el| {
                    el.child(
                        div().flex_1().min_w_0().h_full().child(
                            DataTable::new(&self.report)
                                .xsmall()
                                .bordered(false)
                                .accessibility_label(t!("jobs.report.title").to_string()),
                        ),
                    )
                })
        });
        let current = self.current(cx);
        let running = current.is_some_and(|job| job.status == JobStatus::Running);
        let stopping = current.is_some_and(|job| job.status == JobStatus::Stopping);
        let done = current.is_some_and(|job| !job.is_running());
        let no_report = !current.and_then(Job::report).is_some_and(|r| r.total() > 0);
        let key_context = gated(
            JOBS,
            &[
                (!running, &CancelJob),
                (!stopping, &StopWaiting),
                (!done, &DismissJob),
                (no_report, &SaveReport),
                (no_report, &SaveFailedLines),
            ],
        );
        v_flex()
            .id("jobs.panel")
            .test_support()
            .track_focus(&self.focus_handle)
            .key_context(key_context)
            .size_full()
            .map(|el| match body {
                None => {
                    let text = t!("jobs.empty").to_string();
                    el.child(
                        div()
                            .id("jobs.empty")
                            .test_support()
                            .aria_label(SharedString::from(text.clone()))
                            .p_3()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(text),
                    )
                }
                Some(body) => el.child(
                    v_resizable("jobs.split")
                        .child(
                            resizable_panel().child(
                                DataTable::new(&self.table)
                                    .xsmall()
                                    .bordered(false)
                                    .accessibility_label(t!("jobs.title").to_string()),
                            ),
                        )
                        .child(
                            resizable_panel()
                                .size(px(DETAIL_HEIGHT))
                                .border_t_1()
                                .border_color(cx.theme().border)
                                .child(body),
                        ),
                ),
            })
            .on_action(cx.listener(|this, _: &CancelJob, _, cx| this.cancel(cx)))
            .on_action(cx.listener(|this, _: &StopWaiting, _, cx| this.stop_waiting(cx)))
            .on_action(cx.listener(|this, _: &DismissJob, window, cx| this.dismiss(window, cx)))
            .on_action(cx.listener(|this, _: &SaveReport, window, cx| this.save(false, window, cx)))
            .on_action(
                cx.listener(|this, _: &SaveFailedLines, window, cx| this.save(true, window, cx)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eta_reads_as_a_clock() {
        assert_eq!(eta_text(Duration::from_secs(22)), "0:22");
        assert_eq!(eta_text(Duration::from_secs(125)), "2:05");
        assert_eq!(eta_text(Duration::from_secs(3725)), "1:02:05");
    }
}
