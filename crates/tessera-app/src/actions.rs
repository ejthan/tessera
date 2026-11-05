// SPDX-License-Identifier: GPL-3.0-or-later

//! App-wide actions and their key bindings.

use gpui_kit::component::dock::{ClosePanel, ToggleZoom};
use gpui_kit::{App, KeyBinding, NoAction};

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
        CopyAsCurl,
        TogglePolling,
        Refresh,
        NewCollection,
        ToggleFavorite,
        FocusSearch,
        OpenCollectionRow,
        OpenLink,
        LockAgain,
        CloneCollection,
        ShowForm,
        ShowJson,
        AddField,
        SubmitSchema
    ]
);

/// Key context of a sidebar entry.
pub const SIDEBAR_ENTRY: &str = "SidebarEntry";
/// Key context of the button under a sidebar entry ("Add API key", "Save as profile").
pub const SIDEBAR_ENTRY_BUTTON: &str = "SidebarEntryButton";
/// Key context of the connection dialog.
pub const CONNECT_DIALOG: &str = "ConnectDialog";
/// Key context of an overview tab.
pub const OVERVIEW: &str = "Overview";
/// Key context of a collections tab.
pub const COLLECTIONS: &str = "Collections";
/// Key context of a collection tab.
pub const COLLECTION: &str = "Collection";
/// Key context of the collections table; Enter opens the selected row only there, so a
/// focused header keeps Enter for sorting.
pub const COLLECTIONS_TABLE: &str = "Collections > DataTable";
/// Key context of a sortable header of the collections table: Enter sorts there.
pub const COLLECTIONS_HEADER: &str = "CollectionsHeader";
/// Key context of the Collections and favorite rows under a sidebar entry.
pub const SIDEBAR_LINK: &str = "SidebarLink";
/// Key context of the create and clone dialog.
pub const SCHEMA_DIALOG: &str = "SchemaDialog";
/// Key context of the dialog's last field row, where ⌘↩ / Ctrl+↩ adds the next field.
pub const SCHEMA_LAST_FIELD: &str = "SchemaLastField";

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
const REFRESH: &str = "cmd-r";
#[cfg(not(target_os = "macos"))]
const REFRESH: &str = "ctrl-r";
#[cfg(target_os = "macos")]
const TOGGLE_FAVORITE: &str = "cmd-d";
#[cfg(not(target_os = "macos"))]
const TOGGLE_FAVORITE: &str = "ctrl-d";
#[cfg(target_os = "macos")]
const FIND: &str = "cmd-f";
#[cfg(not(target_os = "macos"))]
const FIND: &str = "ctrl-f";
#[cfg(target_os = "macos")]
const LOCK_AGAIN: &str = "cmd-shift-l";
#[cfg(not(target_os = "macos"))]
const LOCK_AGAIN: &str = "ctrl-shift-l";
#[cfg(target_os = "macos")]
const NEW_COLLECTION: &str = "cmd-shift-n";
#[cfg(not(target_os = "macos"))]
const NEW_COLLECTION: &str = "ctrl-shift-n";
#[cfg(target_os = "macos")]
const CLONE: &str = "alt-cmd-d";
#[cfg(not(target_os = "macos"))]
const CLONE: &str = "ctrl-alt-d";
#[cfg(target_os = "macos")]
const SHOW_FORM: &str = "cmd-1";
#[cfg(not(target_os = "macos"))]
const SHOW_FORM: &str = "ctrl-1";
#[cfg(target_os = "macos")]
const SHOW_JSON: &str = "cmd-2";
#[cfg(not(target_os = "macos"))]
const SHOW_JSON: &str = "ctrl-2";
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
        // Space is text in an input, so it pauses only outside one.
        KeyBinding::new("space", TogglePolling, Some("Overview && !Input")),
        KeyBinding::new(REFRESH, Refresh, Some(OVERVIEW)),
        KeyBinding::new(REFRESH, Refresh, Some(COLLECTIONS)),
        KeyBinding::new(REFRESH, Refresh, Some(COLLECTION)),
        KeyBinding::new(LOCK_AGAIN, LockAgain, Some(OVERVIEW)),
        KeyBinding::new(LOCK_AGAIN, LockAgain, Some(COLLECTIONS)),
        KeyBinding::new(LOCK_AGAIN, LockAgain, Some(COLLECTION)),
        KeyBinding::new(TOGGLE_FAVORITE, ToggleFavorite, Some(COLLECTIONS)),
        KeyBinding::new(FIND, FocusSearch, Some(COLLECTIONS)),
        KeyBinding::new("enter", OpenCollectionRow, Some(COLLECTIONS_TABLE)),
        // Unbinds Enter under the table's binding, so the header handles it as a click.
        KeyBinding::new("enter", NoAction, Some(COLLECTIONS_HEADER)),
        KeyBinding::new("enter", OpenLink, Some(SIDEBAR_LINK)),
        KeyBinding::new(NEW_COLLECTION, NewCollection, Some(COLLECTIONS)),
        KeyBinding::new(CLONE, CloneCollection, Some(COLLECTIONS)),
        KeyBinding::new(CLONE, CloneCollection, Some(COLLECTION)),
        KeyBinding::new(SHOW_FORM, ShowForm, Some(SCHEMA_DIALOG)),
        KeyBinding::new(SHOW_JSON, ShowJson, Some(SCHEMA_DIALOG)),
        KeyBinding::new(SAVE, SubmitSchema, Some(SCHEMA_DIALOG)),
        // Deeper than the dialog, so in the last row ⌘↩ adds a field instead of creating.
        KeyBinding::new(SAVE, AddField, Some(SCHEMA_LAST_FIELD)),
    ]);
    cx.on_action(|_: &Quit, cx| cx.quit());
}
