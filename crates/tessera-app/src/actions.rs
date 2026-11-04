// SPDX-License-Identifier: GPL-3.0-or-later

//! App-wide actions and their key bindings.

use gpui_kit::component::dock::{ClosePanel, ToggleZoom};
use gpui_kit::{App, KeyBinding};

gpui_kit::actions!(
    tessera,
    [
        NewConnection,
        Quit,
        ConnectSelected,
        DisconnectSelected,
        EditConnection,
        DeleteConnection,
        ImportProfiles,
        ExportProfiles,
        ToggleReopenLast,
        TestConnection,
        SaveConnection,
        ToggleKeyMask,
        ToggleRequestLog,
        CopyAsCurl
    ]
);

/// Key context of a sidebar entry.
pub const SIDEBAR_ENTRY: &str = "SidebarEntry";
/// Key context of the button under a sidebar entry ("Add API key", "Save as profile").
pub const SIDEBAR_ENTRY_BUTTON: &str = "SidebarEntryButton";
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
const DELETE: &str = "cmd-backspace";
#[cfg(not(target_os = "macos"))]
const DELETE: &str = "ctrl-backspace";
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
const TOGGLE_LOG: &str = "cmd-j";
#[cfg(not(target_os = "macos"))]
const TOGGLE_LOG: &str = "ctrl-j";
#[cfg(target_os = "macos")]
const COPY_CURL: &str = "cmd-shift-c";
#[cfg(not(target_os = "macos"))]
const COPY_CURL: &str = "ctrl-shift-c";
#[cfg(target_os = "macos")]
const CLOSE_TAB: &str = "cmd-w";
#[cfg(not(target_os = "macos"))]
const CLOSE_TAB: &str = "ctrl-w";
#[cfg(target_os = "macos")]
const ZOOM: &str = "cmd-shift-enter";
#[cfg(not(target_os = "macos"))]
const ZOOM: &str = "ctrl-shift-enter";
#[cfg(target_os = "macos")]
const QUIT: &str = "cmd-q";
#[cfg(not(target_os = "macos"))]
const QUIT: &str = "ctrl-q";

/// Registers global key bindings and app-level action handlers.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new(NEW_CONNECTION, NewConnection, None),
        KeyBinding::new(QUIT, Quit, None),
        KeyBinding::new(TOGGLE_LOG, ToggleRequestLog, None),
        // Global, so the selected log entry can be copied wherever focus is.
        KeyBinding::new(COPY_CURL, CopyAsCurl, None),
        // The dock's own actions, handled by the tab group that holds focus.
        KeyBinding::new(CLOSE_TAB, ClosePanel, None),
        KeyBinding::new(ZOOM, ToggleZoom, None),
        KeyBinding::new("enter", ConnectSelected, Some(SIDEBAR_ENTRY)),
        KeyBinding::new(DISCONNECT, DisconnectSelected, Some(SIDEBAR_ENTRY)),
        KeyBinding::new(EDIT, EditConnection, Some(SIDEBAR_ENTRY)),
        KeyBinding::new(DELETE, DeleteConnection, Some(SIDEBAR_ENTRY)),
        KeyBinding::new(EDIT, EditConnection, Some(SIDEBAR_ENTRY_BUTTON)),
        KeyBinding::new(TEST, TestConnection, Some(CONNECT_DIALOG)),
        KeyBinding::new(SAVE, SaveConnection, Some(CONNECT_DIALOG)),
        KeyBinding::new(TOGGLE_KEY, ToggleKeyMask, Some(CONNECT_DIALOG)),
    ]);
    cx.on_action(|_: &Quit, cx| cx.quit());
}
