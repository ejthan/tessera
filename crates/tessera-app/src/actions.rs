// SPDX-License-Identifier: GPL-3.0-or-later

//! App-wide actions and their key bindings.

use gpui_kit::{App, KeyBinding};

gpui_kit::actions!(
    tessera,
    [
        NewConnection,
        Quit,
        ConnectSelected,
        DisconnectSelected,
        EditConnection,
        TestConnection,
        SaveConnection,
        ToggleKeyMask
    ]
);

/// Key context of a sidebar entry.
pub const SIDEBAR_ENTRY: &str = "SidebarEntry";
/// Key context of the "Add API key" button under a sidebar entry.
pub const SIDEBAR_ADD_KEY: &str = "SidebarAddKey";
/// Key context of the connection dialog.
pub const CONNECT_DIALOG: &str = "ConnectDialog";

#[cfg(target_os = "macos")]
const NEW_CONNECTION: &str = "cmd-n";
#[cfg(not(target_os = "macos"))]
const NEW_CONNECTION: &str = "ctrl-n";
#[cfg(target_os = "macos")]
const DISCONNECT: &str = "cmd-shift-d";
#[cfg(not(target_os = "macos"))]
const DISCONNECT: &str = "ctrl-shift-d";
#[cfg(target_os = "macos")]
const EDIT: &str = "cmd-e";
#[cfg(not(target_os = "macos"))]
const EDIT: &str = "ctrl-e";
#[cfg(target_os = "macos")]
const TEST: &str = "cmd-t";
#[cfg(not(target_os = "macos"))]
const TEST: &str = "ctrl-t";
#[cfg(target_os = "macos")]
const SAVE: &str = "cmd-enter";
#[cfg(not(target_os = "macos"))]
const SAVE: &str = "ctrl-enter";
#[cfg(target_os = "macos")]
const TOGGLE_KEY: &str = "alt-cmd-k";
#[cfg(not(target_os = "macos"))]
const TOGGLE_KEY: &str = "ctrl-alt-k";
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
        KeyBinding::new(EDIT, EditConnection, Some(SIDEBAR_ENTRY)),
        KeyBinding::new(EDIT, EditConnection, Some(SIDEBAR_ADD_KEY)),
        KeyBinding::new(TEST, TestConnection, Some(CONNECT_DIALOG)),
        KeyBinding::new(SAVE, SaveConnection, Some(CONNECT_DIALOG)),
        KeyBinding::new(TOGGLE_KEY, ToggleKeyMask, Some(CONNECT_DIALOG)),
    ]);
    cx.on_action(|_: &Quit, cx| cx.quit());
}
