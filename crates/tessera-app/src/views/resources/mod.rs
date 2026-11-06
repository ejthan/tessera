// SPDX-License-Identifier: GPL-3.0-or-later

//! The views of the relevance resources, one tab per connection and kind.

use gpui_kit::{
    Action, AnyView, App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement,
    ParentElement as _, Render, Styled as _, Window, div, prelude::*,
};
use rust_i18n::t;

use crate::connection::ConnectionState;
use crate::state::{AppState, ResourceKind};

/// The view inside a resource tab.
pub enum ResourceView {
    /// A kind whose view has not arrived: no sidebar row, shortcut or palette entry reaches it.
    Pending(Entity<PendingView>),
}

impl ResourceView {
    /// The view of `kind`.
    pub fn new(
        kind: ResourceKind,
        _connection: Entity<ConnectionState>,
        _state: Entity<AppState>,
        _window: &mut Window,
        cx: &mut App,
    ) -> Self {
        Self::Pending(cx.new(|cx| PendingView {
            kind,
            focus_handle: cx.focus_handle(),
        }))
    }

    /// What it draws.
    pub fn view(&self) -> AnyView {
        match self {
            Self::Pending(view) => view.clone().into(),
        }
    }

    /// Its focus: its shortcuts work from the moment the tab shows.
    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self {
            Self::Pending(view) => view.read(cx).focus_handle.clone(),
        }
    }

    /// Actions with no target now, for [`gated`](crate::actions::gated).
    pub fn gates(&self, _cx: &App) -> Vec<(bool, &'static dyn Action)> {
        Vec::new()
    }
}

/// See [`ResourceView::Pending`].
pub struct PendingView {
    kind: ResourceKind,
    focus_handle: FocusHandle,
}

impl Focusable for PendingView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PendingView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .track_focus(&self.focus_handle)
            .size_full()
            .p_4()
            .child(t!(self.kind.title_key()).to_string())
    }
}
