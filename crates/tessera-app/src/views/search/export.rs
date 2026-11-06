// SPDX-License-Identifier: GPL-3.0-or-later

//! Copying the search as code: the Export menu and "Copied".

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::{Disableable as _, Sizable as _};
use gpui_kit::{
    AnyElement, ClipboardItem, Context, DismissEvent, Entity, Focusable as _, IntoElement,
    ParentElement as _, SharedString, Subscription, Task, Window, div, prelude::*,
};
use rust_i18n::t;
use tessera_client::{Error, SearchRoute};
use tessera_core::search::export::{ExportLang, ExportTarget, export, route_of};
use tessera_core::search::params::{ParamError, to_pairs};

use super::super::schema_view::COPIED_FOR;
use super::multi::multi_request;
use super::{SearchWorkbench, param_error_text};
use crate::actions::{CopyCurl, OpenExport, WORKBENCH};
use crate::connection::error_text;

/// The menu's and "Copied"'s state in the workbench.
#[derive(Default)]
pub(super) struct ExportState {
    menu: Option<(Entity<PopupMenu>, Subscription)>,
    /// Takes "Copied" away again; a newer copy replaces it.
    copied_reset: Option<Task<()>>,
}

/// Why a search cannot be exported.
enum Refused {
    Params(ParamError),
    Route(Error),
}

impl SearchWorkbench {
    /// Hands the export target of the drafts, with its route, to `use_route`.
    fn with_route<R>(
        &self,
        use_route: impl FnOnce(&SearchRoute, &ExportTarget) -> R,
    ) -> Result<R, Refused> {
        let route_and_use = |target: ExportTarget| {
            let route = route_of(&target).map_err(Refused::Route)?;
            Ok(use_route(&route, &target))
        };
        if let [draft] = &self.drafts[..] {
            let params = to_pairs(&draft.params).map_err(Refused::Params)?;
            return route_and_use(ExportTarget::Single {
                collection: &draft.collection,
                params: &params,
            });
        }
        let request = multi_request(&self.drafts, self.union).map_err(Refused::Params)?;
        route_and_use(ExportTarget::Multi(&request))
    }

    fn show_refusal(&mut self, refused: &Refused, cx: &mut Context<Self>) {
        let profile = &self.connection.read(cx).profile;
        let text = match refused {
            Refused::Params(error) => param_error_text(error),
            Refused::Route(error) => error_text(error, profile),
        };
        self.error = Some(text.into());
        cx.notify();
    }

    /// Writes the search as `lang` to the clipboard and says so.
    pub(super) fn copy_export(&mut self, lang: ExportLang, cx: &mut Context<Self>) {
        if self.run_blocked() {
            return;
        }
        let Some(connected) = &self.connection.read(cx).connected else {
            return;
        };
        let node = connected.client.base_url().clone();
        match self.with_route(|route, target| export(lang, &node, route, target)) {
            Ok(Some(code)) => {
                cx.write_to_clipboard(ClipboardItem::new_string(code));
                self.export.copied_reset = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(COPIED_FOR).await;
                    this.update(cx, |this, cx| {
                        this.export.copied_reset = None;
                        cx.notify();
                    })
                    .ok();
                }));
                cx.notify();
            }
            Ok(None) => {}
            Err(refused) => self.show_refusal(&refused, cx),
        }
    }

    pub(super) fn toggle_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.export.menu.take().is_some() {
            self.focus_handle.focus(window, cx);
            return cx.notify();
        }
        if self.run_blocked() {
            return;
        }
        let post = match self.with_route(|route, _| matches!(route, SearchRoute::Post { .. })) {
            Ok(post) => post,
            Err(refused) => return self.show_refusal(&refused, cx),
        };
        let view = cx.entity();
        let target = self.focus_handle.clone();
        let menu = PopupMenu::build(window, cx, move |menu, _, _| {
            let entry = |key: &str, lang: ExportLang| {
                let view = view.clone();
                PopupMenuItem::new(t!(key).to_string()).on_click(move |_, _, cx| {
                    view.update(cx, |this, cx| this.copy_export(lang, cx));
                })
            };
            let raw = if post {
                entry("search.export.post_only", ExportLang::RawUrl).disabled(true)
            } else {
                entry("search.export.raw_url", ExportLang::RawUrl)
            };
            menu.action_context(target)
                .menu(t!("search.export.curl").to_string(), Box::new(CopyCurl))
                .item(entry("search.export.javascript", ExportLang::JavaScript))
                .item(entry("search.export.python", ExportLang::Python))
                .item(entry("search.export.rust", ExportLang::Rust))
                .item(raw)
        });
        menu.focus_handle(cx).focus(window, cx);
        let closed = cx.subscribe_in(&menu, window, |this, _, _: &DismissEvent, window, cx| {
            this.export.menu = None;
            // The menu's focus goes with it, and the tab's shortcuts need one inside the tab.
            this.focus_handle.focus(window, cx);
            cx.notify();
        });
        self.export.menu = Some((menu, closed));
        cx.notify();
    }

    /// The Export button with its menu.
    pub(super) fn export_button(&self, window: &Window, cx: &mut Context<Self>) -> Popover {
        let trigger = Button::new("search.export")
            .ghost()
            .small()
            .label(t!("search.export").to_string())
            .children(Kbd::binding_for_action(
                &OpenExport,
                Some(WORKBENCH),
                window,
            ))
            .disabled(self.run_blocked());
        let menu = self.export.menu.as_ref().map(|(menu, _)| menu.clone());
        let view = cx.entity();
        Popover::new("search.export.popover")
            .appearance(false)
            .overlay_closable(false)
            .open(menu.is_some())
            .on_open_change(move |open, window, cx| {
                view.update(cx, |this, cx| {
                    if *open != this.export.menu.is_some() {
                        this.toggle_export(window, cx);
                    }
                });
            })
            .trigger(trigger)
            .content(move |_, _, _| -> AnyElement {
                match &menu {
                    Some(menu) => menu.clone().into_any_element(),
                    None => div().into_any_element(),
                }
            })
    }

    /// "Copied", for a moment after a copy.
    pub(super) fn export_note(&self) -> Option<impl IntoElement + use<>> {
        self.export.copied_reset.as_ref()?;
        let note = SharedString::from(t!("search.export.copied").to_string());
        Some(
            div()
                .id("search.export.copied")
                .test_support()
                .aria_label(note.clone())
                .text_sm()
                .child(note),
        )
    }
}
