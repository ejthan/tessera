// SPDX-License-Identifier: GPL-3.0-or-later

//! App-wide actions and their key bindings.

use gpui_kit::{App, KeyBinding};

gpui_kit::actions!(tessera, [NewConnection, Quit]);

#[cfg(target_os = "macos")]
const NEW_CONNECTION: &str = "cmd-n";
#[cfg(not(target_os = "macos"))]
const NEW_CONNECTION: &str = "ctrl-n";
#[cfg(target_os = "macos")]
const QUIT: &str = "cmd-q";
#[cfg(not(target_os = "macos"))]
const QUIT: &str = "ctrl-q";

/// Shortcut label shown next to the "Add connection" control.
pub const NEW_CONNECTION_HINT: &str = if cfg!(target_os = "macos") {
    "⌘N"
} else {
    "Ctrl+N"
};

/// Registers global key bindings and app-level action handlers.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new(NEW_CONNECTION, NewConnection, None),
        KeyBinding::new(QUIT, Quit, None),
    ]);
    cx.on_action(|_: &Quit, cx| cx.quit());
}
