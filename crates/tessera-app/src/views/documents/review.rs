// SPDX-License-Identifier: GPL-3.0-or-later

//! The review before a document is saved: target, diff and the exact request body. Medium
//! danger, so focus starts on Back (UX rule 3).

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::Cancel;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Task, Window, div, prelude::*, px,
};
use rust_i18n::t;
use serde_json::Value;
use tessera_core::documents::edit::DiffEntry;
use tessera_core::profile::Profile;
use tessera_widgets::JsonEditor;

use super::copy::pretty;
use super::inspector::SaveMode;
use crate::actions::{ApplyDocument, DOCUMENT_REVIEW};
use crate::views::env_badge;

const WIDTH: f32 = 640.;
const BODY_HEIGHT: f32 = 200.;
const DIFF_HEIGHT: f32 = 240.;

/// Sends the reviewed body and says why it failed.
pub type OnApply = Rc<dyn Fn(&mut Window, &mut App) -> Task<Result<(), SharedString>>>;

/// What the review shows and sends.
pub struct ReviewData {
    /// The connection, for the environment in the title.
    pub profile: Profile,
    /// The collection of the document.
    pub collection: String,
    /// The id of the document.
    pub id: String,
    /// How the document is saved.
    pub mode: SaveMode,
    /// What changes.
    pub entries: Vec<DiffEntry>,
    /// The request body.
    pub body: Value,
    /// The document changed on the server since it was loaded.
    pub stale: bool,
    /// Sends the body.
    pub on_apply: OnApply,
}

struct Review {
    data: ReviewData,
    text: SharedString,
    body: Entity<JsonEditor>,
    back_focus: FocusHandle,
    apply_focus: FocusHandle,
    /// The write running; the dialog stays until it answers.
    task: Option<Task<()>>,
    error: Option<SharedString>,
}

/// Opens the review.
pub fn open(data: ReviewData, window: &mut Window, cx: &mut App) {
    let title = t!("documents.review.title", id = data.id).to_string();
    let profile = data.profile.clone();
    let view = cx.new(|cx| {
        let text = pretty(&data.body);
        Review {
            body: cx.new(|cx| JsonEditor::new(&text, true, window, cx)),
            text: text.into(),
            data,
            back_focus: cx.focus_handle(),
            apply_focus: cx.focus_handle(),
            task: None,
            error: None,
        }
    });
    let first_build = Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if first_build.replace(false) {
            view.read(cx).back_focus.clone().focus(window, cx);
        }
        let (enter, cancel) = (view.clone(), view.clone());
        dialog
            .title(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(title.clone())
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(profile.name.clone()),
                    )
                    .child(env_badge::render(&profile.environment, cx)),
            )
            .w(px(WIDTH))
            // The dialog's Enter binding wins over the focused button's own key handling.
            .on_ok(move |_, window, cx| enter.update(cx, |r, cx| r.press_focused(window, cx)))
            // Dropping a running write would leave its outcome unknown.
            .on_cancel(move |_, _, cx| cancel.read(cx).task.is_none())
            .child(view.clone())
    });
}

impl Review {
    /// Enter: runs Apply when it holds the focus. Says whether the dialog closes.
    fn press_focused(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.apply_focus.is_focused(window) {
            self.apply(window, cx);
            return false;
        }
        self.task.is_none()
    }

    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.task.is_some() {
            return;
        }
        self.error = None;
        let sending = (self.data.on_apply)(window, cx);
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = sending.await;
            this.update_in(cx, |this, window, cx| {
                this.task = None;
                match result {
                    Ok(()) => window.close_dialog(cx),
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn diff_row(&self, entry: &DiffEntry, cx: &App) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (added, removed, mono) = (theme.success, theme.danger, theme.mono_font_family.clone());
        let cell = |value: Option<&Value>, color| {
            value.map(|v| div().min_w_0().text_color(color).child(v.to_string()))
        };
        let (old, new) = (entry.old.as_ref(), entry.new.as_ref());
        let sign = match (old, new) {
            (None, _) => "+ ",
            (_, None) => "− ",
            _ => "",
        };
        let parts: Vec<String> = old.into_iter().chain(new).map(Value::to_string).collect();
        let text = format!("{sign}{}: {}", entry.path, parts.join(" → "));
        h_flex()
            .id(SharedString::from(format!("review.diff.{}", entry.path)))
            .test_support()
            .aria_label(SharedString::from(text))
            .gap_3()
            .items_start()
            .text_sm()
            .font_family(mono)
            .child(div().flex_none().child(format!("{sign}{}", entry.path)))
            .children(cell(old, removed))
            .children(cell(new, added))
    }
}

impl Render for Review {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let d = &self.data;
        let theme = cx.theme();
        let (warning, danger, muted) = (theme.warning, theme.danger, theme.muted_foreground);
        let target = match d.mode {
            SaveMode::Patch => {
                t!(
                    "documents.review.target.patch",
                    collection = d.collection,
                    id = d.id
                )
            }
            SaveMode::Upsert => {
                t!("documents.review.target.upsert", collection = d.collection)
            }
        };
        let notice = |id: &'static str, text: String, color| {
            let text = SharedString::from(text);
            div()
                .id(id)
                .test_support()
                .aria_label(text.clone())
                .text_sm()
                .text_color(color)
                .child(text)
        };
        let running = self.task.is_some();
        let rows: Vec<_> = d.entries.iter().map(|e| self.diff_row(e, cx)).collect();
        v_flex()
            .key_context(DOCUMENT_REVIEW)
            .on_action(cx.listener(|this, _: &ApplyDocument, window, cx| this.apply(window, cx)))
            .gap_3()
            .child(notice("review.target", target.to_string(), muted))
            .when(d.stale, |el| {
                el.child(notice(
                    "review.stale",
                    t!("documents.review.stale").to_string(),
                    warning,
                ))
            })
            .child(
                v_flex()
                    .id("review.diff")
                    .gap_1()
                    .max_h(px(DIFF_HEIGHT))
                    .overflow_y_scroll()
                    .children(rows),
            )
            .when(d.mode == SaveMode::Upsert, |el| {
                el.child(notice(
                    "review.consequence",
                    t!("documents.review.upsert_consequence").to_string(),
                    warning,
                ))
            })
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(t!("documents.review.body").to_string()),
            )
            .child(
                div()
                    .id("review.body")
                    .test_support()
                    .aria_label(self.text.clone())
                    .h(px(BODY_HEIGHT))
                    .child(self.body.clone()),
            )
            .children(
                self.error
                    .clone()
                    .map(|e| notice("review.error", e.to_string(), danger)),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("review.back")
                            .label(t!("documents.review.back").to_string())
                            .children(Kbd::binding_for_action(&Cancel, Some("Dialog"), window))
                            .disabled(running)
                            .track_focus(&self.back_focus)
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("review.apply")
                            .danger()
                            .label(t!("documents.review.apply").to_string())
                            .children(Kbd::binding_for_action(
                                &ApplyDocument,
                                Some(DOCUMENT_REVIEW),
                                window,
                            ))
                            .disabled(running)
                            .track_focus(&self.apply_focus)
                            .on_click(cx.listener(|this, _, window, cx| this.apply(window, cx))),
                    ),
            )
    }
}
