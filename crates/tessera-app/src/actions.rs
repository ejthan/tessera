// SPDX-License-Identifier: GPL-3.0-or-later

//! App-wide actions and their key bindings.

use gpui_kit::{App, KeyBinding};

gpui_kit::actions!(
    tessera,
    [NewConnection, Quit, ConnectSelected, DisconnectSelected]
);

/// Key context of a sidebar entry.
pub const SIDEBAR_ENTRY: &str = "SidebarEntry";

#[cfg(target_os = "macos")]
const NEW_CONNECTION: &str = "cmd-n";
#[cfg(not(target_os = "macos"))]
const NEW_CONNECTION: &str = "ctrl-n";
#[cfg(target_os = "macos")]
const DISCONNECT: &str = "cmd-shift-d";
#[cfg(not(target_os = "macos"))]
const DISCONNECT: &str = "ctrl-shift-d";
#[cfg(target_os = "macos")]
const QUIT: &str = "cmd-q";
#[cfg(not(target_os = "macos"))]
const QUIT: &str = "ctrl-q";

/// Registers global key bindings and app-level action handlers.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new(NEW_CONNECTION, NewConnection, None),
        KeyBinding::new(QUIT, Quit, None),
        KeyBinding::new("enter", ConnectSelected, Some(SIDEBAR_ENTRY)),
        KeyBinding::new(DISCONNECT, DisconnectSelected, Some(SIDEBAR_ENTRY)),
    ]);
    cx.on_action(|_: &Quit, cx| cx.quit());
}
