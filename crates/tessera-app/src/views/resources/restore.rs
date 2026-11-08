// SPDX-License-Identifier: GPL-3.0-or-later

//! The Restore view: choose a backup, see what it would do, and start. The run is a job, so it
//! goes on with the tab closed (D91).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, h_flex, v_flex};
use gpui_kit::{
    Action, App, Context, Entity, FocusHandle, Focusable, FontWeight, Hsla, IntoElement,
    ParentElement as _, PathPromptOptions, Render, SharedString, Styled as _, Subscription, Task,
    Window, div, prelude::*, px,
};
use rust_i18n::t;
use tempfile::TempDir;
use tessera_core::backup::archive::Verified;
use tessera_core::backup::restore::{
    Loaded, PlanRow, RestoreBlock, RestoreJob, RestoreOutcome, RestorePlan, RestoreWarning,
    RowKind, RowResult, RowState,
};
use tessera_core::transfer::{DEFAULT_BATCH, JobControl};

use super::{Danger, client_of, confirm_write, notice, row_button};
use crate::actions::{PickImportFile, SaveResource, StopWaiting};
use crate::connection::{ConnectionState, error_text};
use crate::jobs::{JobId, JobOutcome, JobsGlobal, row_failure_text, row_kind_key};
use crate::transfer::{TransferBackend, TransferBackendGlobal, archive_error_text};
use crate::views::overview::grouped;

/// The most rows drawn.
///
/// shortcut: flex rows, which a real backup never fills; a hostile manifest of 100,000 names
/// would freeze a frame. Move to a virtual list if backups of thousands of collections appear.
const ROW_LIMIT: usize = 1000;

/// How far opening a backup is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Unpacking,
    Verifying,
    Loading,
    Planning,
}

/// A backup that passed every stage.
struct Opened {
    backup: Verified,
    loaded: Loaded,
    plan: RestorePlan,
}

fn warning_text(warning: &RestoreWarning) -> String {
    match warning {
        RestoreWarning::Unstored { collection, fields } => t!(
            "backup.warn.unstored",
            collection = collection,
            fields = fields.join(", ")
        ),
        RestoreWarning::MissingDictionary { collection, name } => t!(
            "restore.warn.dictionary",
            collection = collection,
            name = name
        ),
        RestoreWarning::MissingReference { collection, target } => t!(
            "restore.warn.reference",
            collection = collection,
            target = target
        ),
        RestoreWarning::ReferenceCycle(names) => {
            t!("restore.warn.cycle", names = names.join(", "))
        }
        RestoreWarning::MissingSet {
            collection, name, ..
        } => t!("restore.warn.set", collection = collection, name = name),
        RestoreWarning::AliasNoTarget { alias, target } => {
            t!("restore.warn.alias_target", alias = alias, target = target)
        }
        RestoreWarning::AliasShadowed(alias) => t!("restore.warn.alias_shadowed", alias = alias),
    }
    .to_string()
}

/// The content of the Restore view.
pub struct RestoreForm {
    connection: Entity<ConnectionState>,
    /// Here because dropping the view has no context to find it in.
    backend: Arc<dyn TransferBackend>,
    stage: Option<Stage>,
    /// Stops the stages of opening a backup.
    control: JobControl,
    open_task: Option<Task<()>>,
    opened: Option<Opened>,
    /// The unpacked archive. A running job holds its own handle.
    temp: Option<Arc<TempDir>>,
    /// What was chosen, for the job's row.
    chosen: SharedString,
    ticked: HashSet<(RowKind, String)>,
    replace: bool,
    error: Option<SharedString>,
    /// The job this view started last.
    job: Option<JobId>,
    focus_handle: FocusHandle,
    _subscriptions: [Subscription; 2],
}

impl Drop for RestoreForm {
    fn drop(&mut self) {
        self.control.stop.cancel();
        if let Some(temp) = self.temp.take() {
            self.backend.discard_temp(temp);
        }
    }
}

impl RestoreForm {
    /// The form of `connection`.
    pub fn new(connection: Entity<ConnectionState>, cx: &mut Context<Self>) -> Self {
        let jobs = cx.global::<JobsGlobal>().0.clone();
        Self {
            _subscriptions: [
                cx.observe(&connection, |_, _, cx| cx.notify()),
                cx.observe(&jobs, |_, _, cx| cx.notify()),
            ],
            connection,
            backend: cx.global::<TransferBackendGlobal>().0.clone(),
            stage: None,
            control: JobControl::default(),
            open_task: None,
            opened: None,
            temp: None,
            chosen: SharedString::default(),
            ticked: HashSet::new(),
            replace: false,
            error: None,
            job: None,
            focus_handle: cx.focus_handle(),
        }
    }

    fn choose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: false,
            prompt: Some(t!("restore.choose").to_string().into()),
        });
        self.open_task = Some(cx.spawn_in(window, async move |this, cx| {
            match picked.await {
                Ok(Ok(Some(paths))) => {
                    if let Some(path) = paths.into_iter().next() {
                        this.update(cx, |this, cx| this.open(path, cx)).ok();
                    }
                }
                Ok(Err(e)) => {
                    let text = t!("connect.error.picker", detail = e.to_string()).to_string();
                    this.update(cx, |this, cx| this.fail(text.into(), cx)).ok();
                }
                // Cancelled.
                Ok(Ok(None)) | Err(_) => {}
            }
        }));
    }

    /// Lets go of what is open. A job that uses the temp directory holds it itself.
    fn forget(&mut self) {
        self.opened = None;
        self.ticked.clear();
        self.replace = false;
        if let Some(temp) = self.temp.take() {
            self.backend.discard_temp(temp);
        }
    }

    fn fail(&mut self, text: SharedString, cx: &mut Context<Self>) {
        self.forget();
        self.stage = None;
        self.error = Some(text);
        cx.notify();
    }

    /// Goes through the stages of `path`, an archive or a folder. Each runs off this thread.
    pub fn open(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.control.stop.cancel();
        self.forget();
        self.error = None;
        self.job = None;
        let Some(client) = client_of(&self.connection, cx) else {
            return self.fail(t!("status.not_connected").to_string().into(), cx);
        };
        self.chosen = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
            .into();
        let control = JobControl::default();
        self.control = control.clone();
        self.stage = Some(Stage::Unpacking);
        cx.notify();
        let backend = self.backend.clone();
        let profile = self.connection.read(cx).profile.clone();
        self.open_task = Some(cx.spawn(async move |this, cx| {
            let opened = backend.open_backup(path, control.clone()).await;
            let (folder, manifest, temp) = match opened {
                Ok(opened) => opened,
                Err(e) => {
                    let text = archive_error_text(&e).into();
                    this.update(cx, |this, cx| this.fail(text, cx)).ok();
                    return;
                }
            };
            let stage = |stage| {
                move |this: &mut Self, cx: &mut Context<Self>| {
                    this.stage = Some(stage);
                    cx.notify();
                }
            };
            let alive = this.update(cx, |this, cx| {
                this.temp = temp;
                stage(Stage::Verifying)(this, cx);
            });
            if alive.is_err() {
                return;
            }
            let backup = match backend.verify_backup(folder, manifest, control).await {
                Ok(backup) => backup,
                Err(e) => {
                    let text = archive_error_text(&e).into();
                    this.update(cx, |this, cx| this.fail(text, cx)).ok();
                    return;
                }
            };
            this.update(cx, stage(Stage::Loading)).ok();
            let loaded = match backend.load_backup(backup.clone()).await {
                Ok(loaded) => loaded,
                Err(e) => {
                    let text = t!("restore.error.load", detail = e.to_string())
                        .to_string()
                        .into();
                    this.update(cx, |this, cx| this.fail(text, cx)).ok();
                    return;
                }
            };
            this.update(cx, stage(Stage::Planning)).ok();
            let answer = backend
                .plan_backup(client, backup.clone(), loaded.clone())
                .await;
            this.update(cx, |this, cx| match answer {
                Ok(plan) => this.show(
                    Opened {
                        backup,
                        loaded,
                        plan,
                    },
                    cx,
                ),
                Err(e) => this.fail(error_text(&e, &profile).into(), cx),
            })
            .ok();
        }));
    }

    fn show(&mut self, opened: Opened, cx: &mut Context<Self>) {
        let new = opened.plan.rows.iter().filter(|r| r.state == RowState::New);
        self.ticked = new.map(|r| (r.kind, r.name.clone())).collect();
        self.opened = Some(opened);
        self.stage = None;
        cx.notify();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        self.control.stop.cancel();
        self.open_task = None;
        self.stage = None;
        self.forget();
        cx.notify();
    }

    fn set_replace(&mut self, on: bool, cx: &mut Context<Self>) {
        self.replace = on;
        let Some(opened) = &self.opened else {
            return;
        };
        let differing = opened
            .plan
            .rows
            .iter()
            .filter(|r| r.state == RowState::Differs);
        let keys: Vec<_> = differing.map(|r| (r.kind, r.name.clone())).collect();
        for key in keys {
            if on {
                self.ticked.insert(key);
            } else {
                self.ticked.remove(&key);
            }
        }
        cx.notify();
    }

    fn running(&self, cx: &App) -> bool {
        let jobs = cx.global::<JobsGlobal>().0.read(cx).jobs();
        self.job
            .is_some_and(|id| jobs.iter().any(|j| j.id == id && j.is_running()))
    }

    /// The rows that would be written.
    fn writes(&self) -> Vec<&PlanRow> {
        let Some(opened) = &self.opened else {
            return Vec::new();
        };
        let key = |r: &&PlanRow| (r.kind, r.name.clone());
        let rows = opened.plan.rows.iter();
        rows.filter(|r| match r.state {
            RowState::New => self.ticked.contains(&key(r)),
            RowState::Differs => self.replace && self.ticked.contains(&key(r)),
            RowState::Same | RowState::Blocked(_) => false,
        })
        .collect()
    }

    fn can_start(&self, cx: &App) -> bool {
        !self.writes().is_empty() && !self.running(cx)
    }

    /// Actions with no target now, for [`gated`](crate::actions::gated).
    pub fn gates(&self, cx: &App) -> Vec<(bool, &'static dyn Action)> {
        vec![
            (!self.can_start(cx), &SaveResource),
            (self.stage.is_some(), &PickImportFile),
            (self.stage.is_none(), &StopWaiting),
        ]
    }

    /// The lines of the confirmation: each number, and what stays.
    fn lines(&self) -> Vec<String> {
        let writes = self.writes();
        let count = |kinds: &[RowKind], state: &RowState| {
            let rows = writes
                .iter()
                .filter(|r| kinds.contains(&r.kind) && r.state == *state);
            grouped(rows.count() as u64)
        };
        let sets = [RowKind::SynonymSet, RowKind::CurationSet];
        let stopwords = [RowKind::Stopwords];
        let aliases = [RowKind::Alias];
        let Some(opened) = &self.opened else {
            return Vec::new();
        };
        let collections: HashSet<&str> = writes
            .iter()
            .filter(|r| r.kind == RowKind::Collection)
            .map(|r| r.name.as_str())
            .collect();
        let manifest = opened.backup.manifest();
        let picked = manifest
            .collections
            .iter()
            .filter(|c| collections.contains(c.name.as_str()));
        let documents: u64 = picked.map(|c| c.documents).sum();
        let mut lines = vec![
            t!(
                "restore.confirm.collections",
                collections = grouped(collections.len() as u64),
                documents = grouped(documents)
            )
            .to_string(),
            t!(
                "restore.confirm.create",
                sets = count(&sets, &RowState::New),
                stopwords = count(&stopwords, &RowState::New),
                aliases = count(&aliases, &RowState::New)
            )
            .to_string(),
            t!(
                "restore.confirm.replace",
                sets = count(&sets, &RowState::Differs),
                stopwords = count(&stopwords, &RowState::Differs),
                aliases = count(&aliases, &RowState::Differs)
            )
            .to_string(),
            t!("restore.confirm.untouched").to_string(),
        ];
        let blocked = |r: &&PlanRow| matches!(r.state, RowState::Blocked(_));
        let blocked = opened.plan.rows.iter().filter(blocked).count() as u64;
        if blocked > 0 {
            lines.push(t!("restore.confirm.blocked", count = grouped(blocked)).to_string());
        }
        lines
    }

    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_start(cx) {
            return;
        }
        let lines = self.lines();
        let replaces = self.writes().iter().any(|r| r.state == RowState::Differs);
        let (view, connection) = (cx.entity().downgrade(), self.connection.clone());
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                let profile = &connection.read(cx).profile;
                let danger = Danger {
                    title: t!("restore.confirm.title").to_string(),
                    lines,
                    expected: profile.name.clone(),
                    action: t!("restore.start_action").to_string(),
                    typed: profile.environment.is_prod() || replaces,
                };
                let view = view.clone();
                let run = move |_: &mut Window, cx: &mut App| {
                    view.update(cx, |view, cx| view.run(cx)).ok();
                    Task::ready(Ok(()))
                };
                confirm_write(danger, &connection, run, |_, _| {}, window, cx);
            });
        });
    }

    fn run(&mut self, cx: &mut Context<Self>) {
        let Some(opened) = &self.opened else {
            return;
        };
        let job = RestoreJob {
            backup: opened.backup.clone(),
            loaded: opened.loaded.clone(),
            ticked: self.ticked.clone(),
            replace: self.replace,
            batch_size: DEFAULT_BATCH,
        };
        let label = self.chosen.clone();
        let (connection, temp) = (self.connection.clone(), self.temp.clone());
        let jobs = cx.global::<JobsGlobal>().0.clone();
        let id = jobs.update(cx, |jobs, cx| {
            jobs.start_restore(&connection, job, temp, label, cx)
        });
        self.job = Some(id);
        cx.notify();
    }

    /// Why a row cannot be restored, and what the user can do.
    fn block_text(&self, row: &PlanRow, block: &RestoreBlock) -> String {
        match block {
            RestoreBlock::CollectionExists => t!("restore.block.exists").to_string(),
            RestoreBlock::DuplicateName => t!("restore.block.duplicate").to_string(),
            RestoreBlock::Mask => t!("restore.block.mask").to_string(),
            RestoreBlock::RequiredUnstored(fields) => {
                t!("restore.block.unstored", fields = fields.join(", ")).to_string()
            }
            RestoreBlock::BlankSecrets(count) => {
                let opened = self.opened.as_ref();
                let manifest = opened.map(|o| o.backup.manifest());
                let entry =
                    manifest.and_then(|m| m.collections.iter().find(|c| c.name == row.name));
                t!(
                    "restore.block.keys",
                    count = count,
                    schema = entry.map(|e| e.schema_path()).unwrap_or_default(),
                    documents = entry.map(|e| e.documents_path()).unwrap_or_default()
                )
                .to_string()
            }
        }
    }

    fn state_text(row: &PlanRow) -> String {
        match row.state {
            RowState::New => t!("restore.state.new"),
            RowState::Same => t!("restore.state.same"),
            RowState::Differs => t!("restore.state.differs"),
            RowState::Blocked(_) => t!("restore.state.blocked"),
        }
        .to_string()
    }

    /// One row. A blocked or equal row has nothing to tick.
    fn plan_row(&self, row: &PlanRow, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let key = (row.kind, row.name.clone());
        let id = format!("restore.row.{:?}.{}", row.kind, row.name);
        let state = Self::state_text(row);
        let what = t!(row_kind_key(row.kind)).to_string();
        let detail = match &row.state {
            RowState::Blocked(block) => Some(self.block_text(row, block)),
            RowState::Differs if !self.replace => Some(t!("restore.state.kept").to_string()),
            _ => None,
        };
        let label = format!(
            "{what} {} {state} {}",
            row.name,
            detail.clone().unwrap_or_default()
        );
        let checkable = matches!(row.state, RowState::New | RowState::Differs);
        let enabled = row.state == RowState::New || self.replace;
        let tick = checkable.then(|| {
            let checked = self.ticked.contains(&key);
            Checkbox::new(SharedString::from(format!("{id}.tick")))
                .accessibility_label(label.clone())
                .checked(checked)
                .disabled(!enabled)
                .on_click(cx.listener(move |this, on: &bool, _, cx| {
                    if *on {
                        this.ticked.insert(key.clone());
                    } else {
                        this.ticked.remove(&key);
                    }
                    cx.notify();
                }))
        });
        let muted = cx.theme().muted_foreground;
        h_flex()
            .id(SharedString::from(id))
            .test_support()
            .aria_label(SharedString::from(label))
            .gap_2()
            .py_1()
            .items_start()
            .text_sm()
            .child(div().w(px(20.)).children(tick))
            .child(div().w(px(120.)).text_color(muted).child(what))
            .child(div().w(px(240.)).truncate().child(row.name.clone()))
            .child(div().w(px(100.)).child(state))
            .child(div().flex_1().text_color(muted).children(detail))
    }

    fn outcome_section(
        outcome: &RestoreOutcome,
        danger: Hsla,
        muted: Hsla,
    ) -> impl IntoElement + use<> {
        let rows = outcome.rows.iter().take(ROW_LIMIT).map(|row| {
            let (text, color) = match &row.result {
                RowResult::Done => (t!("restore.result.done").to_string(), muted),
                RowResult::Kept => (t!("restore.result.kept").to_string(), muted),
                RowResult::Skipped => (t!("restore.result.skipped").to_string(), muted),
                RowResult::Failed(failure) => (row_failure_text(failure), danger),
            };
            let what = t!(row_kind_key(row.kind));
            notice(
                format!("restore.result.{:?}.{}", row.kind, row.name),
                format!("{what} {}: {text}", row.name).into(),
                color,
            )
        });
        let warnings = outcome.warnings.iter().enumerate().map(|(i, warning)| {
            notice(
                format!("restore.result_warning.{i}"),
                warning_text(warning).into(),
                muted,
            )
        });
        let to_delete = (!outcome.to_delete.is_empty()).then(|| {
            let names = outcome.to_delete.join(", ");
            notice(
                "restore.to_delete",
                t!("restore.to_delete", names = names).to_string().into(),
                danger,
            )
        });
        v_flex()
            .p_2()
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(t!("restore.results").to_string()),
            )
            .children(rows)
            .children(warnings)
            .children(to_delete)
    }

    fn stage_text(stage: Stage) -> String {
        match stage {
            Stage::Unpacking => t!("restore.stage.unpacking"),
            Stage::Verifying => t!("restore.stage.verifying"),
            Stage::Loading => t!("restore.stage.loading"),
            Stage::Planning => t!("restore.stage.planning"),
        }
        .to_string()
    }
}

impl Focusable for RestoreForm {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for RestoreForm {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let locked = self.connection.read(cx).writes_locked();
        let (danger, muted, warning) = {
            let theme = cx.theme();
            (theme.danger, theme.muted_foreground, theme.warning)
        };
        let stage = self.stage;
        let focus = self.focus_handle.clone();
        let warnings: Vec<String> = self
            .opened
            .iter()
            .flat_map(|o| o.plan.warnings.iter().map(warning_text))
            .collect();
        let shown: Vec<PlanRow> = self
            .opened
            .iter()
            .flat_map(|o| o.plan.rows.iter().take(ROW_LIMIT).cloned())
            .collect();
        let total = self.opened.as_ref().map_or(0, |o| o.plan.rows.len());
        let rows: Vec<_> = shown.iter().map(|row| self.plan_row(row, cx)).collect();
        let jobs = cx.global::<JobsGlobal>().0.read(cx);
        let job = self
            .job
            .and_then(|id| jobs.jobs().iter().find(|j| j.id == id));
        let failure = job.and_then(|job| match &job.status {
            crate::jobs::JobStatus::Failed(text) => Some(text.clone()),
            _ => None,
        });
        let outcome = job.and_then(|job| match &job.outcome {
            Some(JobOutcome::Restore(outcome)) => {
                Some(Self::outcome_section(outcome, danger, muted))
            }
            _ => None,
        });
        let has_differs = shown.iter().any(|r| r.state == RowState::Differs);
        let opened = self.opened.is_some();
        div()
            .id("restore.view")
            .track_focus(&self.focus_handle)
            .size_full()
            .overflow_y_scroll()
            .p_2()
            .on_action(cx.listener(|this, _: &PickImportFile, window, cx| {
                this.choose(window, cx);
            }))
            .on_action(cx.listener(|this, _: &SaveResource, window, cx| this.start(window, cx)))
            .on_action(cx.listener(|this, _: &StopWaiting, _, cx| this.stop(cx)))
            .child(
                h_flex()
                    .gap_2()
                    .p_2()
                    .items_center()
                    .child(row_button(
                        "restore.choose".into(),
                        t!("restore.choose").to_string(),
                        &PickImportFile,
                        (stage.is_some(), false),
                        &focus,
                        window,
                        cx,
                    ))
                    .when_some(stage, |el, stage| {
                        el.child(notice(
                            "restore.stage",
                            Self::stage_text(stage).into(),
                            muted,
                        ))
                        .child(row_button(
                            "restore.stop".into(),
                            t!("jobs.cancel").to_string(),
                            &StopWaiting,
                            (false, false),
                            &focus,
                            window,
                            cx,
                        ))
                    }),
            )
            .children(
                self.error
                    .clone()
                    .map(|e| notice("restore.error", e, danger)),
            )
            .children(
                warnings
                    .into_iter()
                    .enumerate()
                    .map(|(i, text)| notice(format!("restore.warning.{i}"), text.into(), warning)),
            )
            .child(v_flex().p_2().children(rows))
            .when(total > ROW_LIMIT, |el| {
                let text = t!(
                    "restore.rows_limited",
                    shown = grouped(ROW_LIMIT as u64),
                    total = grouped(total as u64)
                );
                el.child(notice(
                    "restore.rows_limited",
                    text.to_string().into(),
                    muted,
                ))
            })
            .when(has_differs, |el| {
                el.child(
                    h_flex().p_2().child(
                        Checkbox::new("restore.replace")
                            .label(t!("restore.replace").to_string())
                            .checked(self.replace)
                            .on_click(cx.listener(|this, on: &bool, _, cx| {
                                this.set_replace(*on, cx);
                            })),
                    ),
                )
            })
            .when(opened, |el| {
                el.child(h_flex().p_2().child(row_button(
                    "restore.start".into(),
                    t!("restore.start").to_string(),
                    &SaveResource,
                    (!self.can_start(cx), locked),
                    &focus,
                    window,
                    cx,
                )))
            })
            .children(failure.map(|text| notice("restore.failed", text, danger)))
            .children(outcome)
    }
}
