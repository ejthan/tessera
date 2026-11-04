// SPDX-License-Identifier: GPL-3.0-or-later

//! The bottom status bar.

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::{IntoElement, ParentElement as _, SharedString, div, prelude::*};
use rust_i18n::t;

/// Renders the status bar for the current connection state.
pub fn render() -> impl IntoElement {
    let text = SharedString::from(t!("status.not_connected").to_string());
    StatusBar::new().left(
        div()
            .id("status.text")
            .test_support()
            .aria_label(text.clone())
            .child(text),
    )
}
