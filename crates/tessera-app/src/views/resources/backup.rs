// SPDX-License-Identifier: GPL-3.0-or-later

//! The Backup tab: which collections to write where, and a switch to the Restore view. The run
//! is a job, so it goes on with the tab closed (D90).

use std::collections::HashSet;
use std::path::PathBuf;

use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::radio::{Radio, RadioGroup};
use gpui_kit::component::tab::TabBar;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    Action, App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, Window, div,
    prelude::*,
};
use jiff::Zoned;
use rust_i18n::t;
use tessera_core::backup::run::{BackupPlan, BackupTarget, BackupWarning, backup_warnings};

use super::restore::RestoreForm;
use super::{notice, row_button};
use crate::actions::{PickImportFile, RESOURCES, SaveResource, ShowDocuments, ShowSchema};
use crate::connection::ConnectionState;
use crate::jobs::JobsGlobal;
use crate::transfer::TransferBackendGlobal;
use crate::views::dock::view_tab;
use crate::views::overview::grouped;

/// The Backup tab: ⌘1 writes a backup, ⌘2 restores one, as the AI tab switches.
pub struct BackupView {
    form: Entity<BackupForm>,
    restore: Entity<RestoreForm>,
    on_restore: bool,
}

impl BackupView {
    /// The tab of `connection`.
    pub fn new(connection: Entity<ConnectionState>, cx: &mut Context<Self>) -> Self {
        Self {
            form: cx.new(|cx| BackupForm::new(connection.clone(), cx)),
            restore: cx.new(|cx| RestoreForm::new(connection, cx)),
            on_restore: false,
        }
    }

    /// The Restore view.
    pub fn restore(&self) -> &Entity<RestoreForm> {
        &self.restore
    }

    fn show(&mut self, on_restore: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.on_restore = on_restore;
        self.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    /// Actions with no target now, for [`gated`](crate::actions::gated).
    pub fn gates(&self, cx: &App) -> Vec<(bool, &'static dyn Action)> {
        if self.on_restore {
            self.restore.read(cx).gates(cx)
        } else {
            let mut gates = self.form.read(cx).gates(cx);
            gates.push((true, &PickImportFile));
            gates
        }
    }
}

impl Focusable for BackupView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        if self.on_restore {
            self.restore.read(cx).focus_handle(cx)
        } else {
            self.form.read(cx).focus_handle(cx)
        }
    }
}

impl Render for BackupView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current = if self.on_restore {
            self.restore.clone().into_any_element()
        } else {
            self.form.clone().into_any_element()
        };
        v_flex()
            .size_full()
            .on_action(cx.listener(|this, _: &ShowDocuments, window, cx| {
                this.show(false, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowSchema, window, cx| {
                this.show(true, window, cx);
            }))
            .child(
                TabBar::new("backup.views")
                    .underline()
                    .selected_index(usize::from(self.on_restore))
                    .on_click(cx.listener(|this, ix: &usize, window, cx| {
                        this.show(*ix == 1, window, cx);
                    }))
                    .child(view_tab(
                        "backup.view.backup",
                        t!("backup.view.backup").to_string().into(),
                        &ShowDocuments,
                        RESOURCES,
                        window,
                    ))
                    .child(view_tab(
                        "backup.view.restore",
                        t!("backup.view.restore").to_string().into(),
                        &ShowSchema,
                        RESOURCES,
                        window,
                    )),
            )
            .child(div().flex_1().min_h_0().child(current))
    }
}

/// The sentence for a warning of a backup.
pub(crate) fn warning_text(warning: &BackupWarning) -> String {
    match warning {
        BackupWarning::Unstored { collection, fields } => t!(
            "backup.warn.unstored",
            collection = collection,
            fields = fields.join(", ")
        ),
        BackupWarning::BlankedSecrets { collection, count } => t!(
            "backup.warn.blanked",
            collection = collection,
            count = count
        ),
        BackupWarning::BlankedSetSecrets { name, count, .. } => {
            t!("backup.warn.blanked_set", name = name, count = count)
        }
        BackupWarning::Dictionaries { collection, names } => t!(
            "backup.warn.dictionaries",
            collection = collection,
            names = names.join(", ")
        ),
        BackupWarning::OutsideReference { collection, target } => t!(
            "backup.warn.outside",
            collection = collection,
            target = target
        ),
        BackupWarning::CountMoved {
            collection,
            expected,
            written,
        } => t!(
            "backup.warn.count",
            collection = collection,
            expected = grouped(*expected),
            written = grouped(*written)
        ),
        BackupWarning::MissingSet {
            collection, name, ..
        } => t!(
            "backup.warn.missing_set",
            collection = collection,
            name = name
        ),
    }
    .to_string()
}

/// The content of the Backup view.
pub struct BackupForm {
    connection: Entity<ConnectionState>,
    /// The ticked collections.
    selected: HashSet<String>,
    archive: bool,
    /// The last target refused, or what the picker said.
    error: Option<SharedString>,
    path_task: Option<Task<()>>,
    focus_handle: FocusHandle,
    _subscription: Subscription,
}

impl BackupForm {
    /// The form of `connection`.
    pub fn new(connection: Entity<ConnectionState>, cx: &mut Context<Self>) -> Self {
        connection.update(cx, |c, cx| c.load_collection_list(cx));
        Self {
            _subscription: cx.observe(&connection, |_, _, cx| cx.notify()),
            connection,
            selected: HashSet::new(),
            archive: false,
            error: None,
            path_task: None,
            focus_handle: cx.focus_handle(),
        }
    }

    /// Ticks or unticks every collection, as the All box does.
    pub fn set_all(&mut self, on: bool, cx: &mut Context<Self>) {
        self.selected.clear();
        if on {
            self.selected.extend(self.names(cx));
        }
        cx.notify();
    }

    /// Ticks or unticks `name`, as its box does.
    pub fn set_selected(&mut self, name: String, on: bool, cx: &mut Context<Self>) {
        if on {
            self.selected.insert(name);
        } else {
            self.selected.remove(&name);
        }
        cx.notify();
    }

    fn names(&self, cx: &App) -> Vec<String> {
        let cache = &self.connection.read(cx).collection_cache;
        let collections = cache.collections.iter().flatten();
        collections.map(|c| c.name.clone()).collect()
    }

    /// The ticked names that still exist, in list order.
    fn picked(&self, cx: &App) -> Vec<String> {
        let names = self.names(cx).into_iter();
        names.filter(|n| self.selected.contains(n)).collect()
    }

    /// Asks where to, then starts. A cancelled prompt changes nothing.
    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.picked(cx).is_empty() {
            return;
        }
        self.error = None;
        let dir = std::env::home_dir().unwrap_or_default();
        let name = self.suggested_name(cx);
        let picked = cx.prompt_for_new_path(&dir, Some(&name));
        self.path_task = Some(cx.spawn_in(window, async move |this, cx| {
            match picked.await {
                Ok(Ok(Some(path))) => {
                    this.update(cx, |this, cx| this.check_target(path, cx)).ok();
                }
                Ok(Err(e)) => {
                    let text = t!("connect.error.picker", detail = e.to_string()).to_string();
                    this.update(cx, |this, cx| this.refuse(text.into(), cx))
                        .ok();
                }
                // Cancelled.
                Ok(Ok(None)) | Err(_) => {}
            }
        }));
        cx.notify();
    }

    fn suggested_name(&self, cx: &App) -> String {
        let profile = &self.connection.read(cx).profile.name;
        let date = Zoned::now().strftime("%Y-%m-%d");
        let extension = if self.archive { ".tar.zst" } else { "" };
        format!("{profile}-{date}{extension}")
    }

    fn refuse(&mut self, text: SharedString, cx: &mut Context<Self>) {
        self.error = Some(text);
        cx.notify();
    }

    /// The picker has already asked about replacing, and a backup never replaces. A path that
    /// is taken, by a link that leads nowhere too, is refused here and not in the job.
    fn check_target(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let check = cx
            .global::<TransferBackendGlobal>()
            .0
            .path_taken(path.clone());
        self.path_task = Some(cx.spawn(async move |this, cx| {
            let taken = check.await;
            this.update(cx, |this, cx| {
                if taken {
                    let path = path.display().to_string();
                    this.refuse(
                        t!("backup.target_exists", path = path).to_string().into(),
                        cx,
                    );
                } else {
                    this.run(path, cx);
                }
            })
            .ok();
        }));
    }

    fn run(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let file = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let collections = self.picked(cx);
        let count = collections.len();
        let label = t!("backup.job_label", count = count, file = file);
        let target = if self.archive {
            BackupTarget::Archive(path)
        } else {
            BackupTarget::Folder(path)
        };
        let plan = BackupPlan {
            collections,
            target,
        };
        let connection = self.connection.clone();
        let jobs = cx.global::<JobsGlobal>().0.clone();
        jobs.update(cx, |jobs, cx| {
            jobs.start_backup(&connection, plan, label.to_string().into(), cx);
        });
    }

    /// Actions with no target now, for [`gated`](crate::actions::gated).
    pub fn gates(&self, cx: &App) -> Vec<(bool, &'static dyn Action)> {
        vec![(self.picked(cx).is_empty(), &SaveResource)]
    }

    fn warnings(&self, cx: &App) -> Vec<String> {
        let cache = &self.connection.read(cx).collection_cache;
        let (Some(collections), aliases) = (&cache.collections, &cache.aliases) else {
            return Vec::new();
        };
        let found = backup_warnings(collections, &self.picked(cx), aliases);
        found.iter().map(warning_text).collect()
    }

    fn rows(&self, cx: &mut Context<Self>) -> Vec<impl IntoElement + use<>> {
        let cache = &self.connection.read(cx).collection_cache;
        let counts: Vec<(String, u64)> = (cache.collections.iter().flatten())
            .map(|c| (c.name.clone(), c.num_documents))
            .collect();
        let muted = cx.theme().muted_foreground;
        counts
            .into_iter()
            .map(|(name, count)| {
                let label = t!("backup.row", name = name, count = grouped(count)).to_string();
                let checked = self.selected.contains(&name);
                h_flex().gap_2().py_1().text_color(muted).child(
                    Checkbox::new(SharedString::from(format!("backup.row.{name}")))
                        .label(label.clone())
                        .accessibility_label(label)
                        .checked(checked)
                        .on_click(cx.listener(move |this, on: &bool, _, cx| {
                            this.set_selected(name.clone(), *on, cx);
                        })),
                )
            })
            .collect()
    }
}

impl Focusable for BackupForm {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for BackupForm {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let picked = self.picked(cx);
        let total = self.names(cx).len();
        let all = total > 0 && picked.len() == total;
        let warnings = self.warnings(cx);
        let rows = self.rows(cx);
        let (danger, muted, warning) = {
            let theme = cx.theme();
            (theme.danger, theme.muted_foreground, theme.warning)
        };
        let list_error = {
            let cache = &self.connection.read(cx).collection_cache;
            cache.error.clone()
        };
        div()
            .id("backup.view")
            .track_focus(&self.focus_handle)
            .size_full()
            .overflow_y_scroll()
            .p_2()
            .on_action(cx.listener(|this, _: &SaveResource, window, cx| this.start(window, cx)))
            .child(notice(
                "backup.includes",
                t!("backup.includes").to_string().into(),
                muted,
            ))
            .children(list_error.map(|e| notice("backup.list_error", e, danger)))
            .child(
                v_flex()
                    .p_2()
                    .child(
                        Checkbox::new("backup.all")
                            .label(t!("backup.all").to_string())
                            .checked(all)
                            .on_click(cx.listener(|this, on: &bool, _, cx| this.set_all(*on, cx))),
                    )
                    .children(rows),
            )
            .child(
                v_flex()
                    .gap_1()
                    .p_2()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t!("backup.format").to_string()),
                    )
                    .child(
                        RadioGroup::horizontal("backup.format")
                            .selected_index(Some(usize::from(self.archive)))
                            .on_click(cx.listener(|this, index: &usize, _, cx| {
                                this.archive = *index == 1;
                                cx.notify();
                            }))
                            .child(Radio::new(0).label(t!("backup.format.folder").to_string()))
                            .child(Radio::new(1).label(t!("backup.format.archive").to_string())),
                    ),
            )
            .children(
                warnings
                    .into_iter()
                    .enumerate()
                    .map(|(i, text)| notice(format!("backup.warning.{i}"), text.into(), warning)),
            )
            .children(
                self.error
                    .clone()
                    .map(|e| notice("backup.error", e, danger)),
            )
            .child(h_flex().p_2().child(row_button(
                "backup.start".into(),
                t!("backup.start").to_string(),
                &SaveResource,
                (picked.is_empty(), false),
                &self.focus_handle,
                window,
                cx,
            )))
    }
}
