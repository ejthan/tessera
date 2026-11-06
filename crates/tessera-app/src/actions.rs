// SPDX-License-Identifier: GPL-3.0-or-later

//! App-wide actions and their key bindings.

use gpui_kit::base::actions::{SelectDown, SelectUp};
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
        EditSchema,
        ShowForm,
        ShowJson,
        AddField,
        SubmitSchema,
        TruncateCollection,
        DeleteCollection,
        ShowDocuments,
        ShowSchema,
        SortByColumn,
        ToggleColumns,
        TogglePinColumn,
        NarrowerColumn,
        WiderColumn,
        ClearFilter,
        ToggleInspector,
        OpenDocument,
        GoToId,
        CopyCell,
        CopyRowJson,
        CopyJsonl,
        CopyCsv,
        CopyCurl,
        NewDocument,
        ReviewDocument,
        ToggleSaveMode,
        RevertDocument,
        ApplyDocument,
        ToggleRow,
        ExtendRows,
        SelectLoaded,
        DeleteDocuments,
        DeleteByQuery,
        UpdateByQuery,
        BulkContinue,
        OpenSearchTab,
        RunSearch,
        ShowHits,
        ShowRawResult,
        ToggleInstant,
        FocusQuery,
        NextResultPage,
        PreviousResultPage,
        AddCondition,
        MoveChipLeft,
        MoveChipRight,
        RemoveChip,
        AddSearch,
        RemoveSearch,
        NextSearch,
        PreviousSearch,
        ToggleUnion,
        OpenHistory,
        OpenSaved,
        SaveQuery,
        OpenPresets,
        LoadQuery,
        DeleteSavedQuery
    ]
);

#[cfg(feature = "frame-overlay")]
gpui_kit::actions!(tessera, [ToggleFrameOverlay]);

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
/// Key context of the documents view inside a collection tab.
pub const DOCUMENTS: &str = "Documents";
/// Key context of the documents table.
pub const DOCUMENTS_TABLE: &str = "Documents > DataTable";
/// Key context of the inspector beside the documents table.
pub const INSPECTOR: &str = "Inspector";
/// Key context of the inspector's editor, whose own ⌘↩ / Ctrl+↩ would add a line.
const INSPECTOR_EDITOR: &str = "Inspector > Input";
/// Key context of the delete and update by query dialog.
pub const BULK_DIALOG: &str = "BulkDialog";
/// An input in that dialog, which has its own ⌘↩.
const BULK_INPUT: &str = "BulkDialog > Input";
/// Key context of the document review.
pub const DOCUMENT_REVIEW: &str = "DocumentReview";
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
/// Key context of a search tab.
pub const WORKBENCH: &str = "Workbench";
/// An input in a search tab, which has its own ⌘↩.
const WORKBENCH_INPUT: &str = "Workbench > Input";
/// Key context of the Add condition row, deeper than Run.
pub const CONDITION_ROW: &str = "ConditionRow";
/// The value input of that row.
const CONDITION_INPUT: &str = "ConditionRow > Input";
/// Key context of a list in a search tab's toolbar.
pub const QUERY_LIST: &str = "QueryList";
/// The filter of that list, which keeps the focus while the arrows move the selection.
const QUERY_LIST_INPUT: &str = "QueryList > Input";
/// Key context of the hit list.
pub const SEARCH_HITS: &str = "SearchHits";
/// Key context of a focused `query_by` chip.
pub const CHIP: &str = "SearchChip";

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
// D20: ⌥⌘D hides the Dock on macOS. The same chord as Disconnect, which is bound only in
// SIDEBAR_ENTRY, so the two never meet.
#[cfg(target_os = "macos")]
const CLONE: &str = "cmd-shift-d";
#[cfg(not(target_os = "macos"))]
const CLONE: &str = "ctrl-shift-d";
#[cfg(target_os = "macos")]
const SHOW_FORM: &str = "cmd-1";
#[cfg(not(target_os = "macos"))]
const SHOW_FORM: &str = "ctrl-1";
#[cfg(target_os = "macos")]
const SHOW_JSON: &str = "cmd-2";
#[cfg(not(target_os = "macos"))]
const SHOW_JSON: &str = "ctrl-2";
#[cfg(target_os = "macos")]
const TRUNCATE: &str = "cmd-shift-backspace";
#[cfg(not(target_os = "macos"))]
const TRUNCATE: &str = "ctrl-shift-backspace";
#[cfg(target_os = "macos")]
const SORT: &str = "cmd-shift-s";
#[cfg(not(target_os = "macos"))]
const SORT: &str = "ctrl-shift-s";
#[cfg(target_os = "macos")]
const COLUMNS: &str = "cmd-shift-o";
#[cfg(not(target_os = "macos"))]
const COLUMNS: &str = "ctrl-shift-o";
#[cfg(target_os = "macos")]
const PIN: &str = "cmd-shift-p";
#[cfg(not(target_os = "macos"))]
const PIN: &str = "ctrl-shift-p";
#[cfg(target_os = "macos")]
const NARROWER: &str = "cmd-shift-left";
#[cfg(not(target_os = "macos"))]
const NARROWER: &str = "ctrl-shift-left";
#[cfg(target_os = "macos")]
const WIDER: &str = "cmd-shift-right";
#[cfg(not(target_os = "macos"))]
const WIDER: &str = "ctrl-shift-right";
#[cfg(target_os = "macos")]
const CLEAR_FILTER: &str = "cmd-shift-f";
#[cfg(not(target_os = "macos"))]
const CLEAR_FILTER: &str = "ctrl-shift-f";
#[cfg(target_os = "macos")]
const TOGGLE_INSPECTOR: &str = "cmd-i";
#[cfg(not(target_os = "macos"))]
const TOGGLE_INSPECTOR: &str = "ctrl-i";
#[cfg(target_os = "macos")]
const GO_TO_ID: &str = "cmd-g";
#[cfg(not(target_os = "macos"))]
const GO_TO_ID: &str = "ctrl-g";
#[cfg(target_os = "macos")]
const COPY_CELL: &str = "cmd-c";
#[cfg(not(target_os = "macos"))]
const COPY_CELL: &str = "ctrl-c";
#[cfg(target_os = "macos")]
const COPY_ROW: &str = "cmd-alt-c";
#[cfg(not(target_os = "macos"))]
const COPY_ROW: &str = "ctrl-alt-c";
#[cfg(target_os = "macos")]
const COPY_JSONL: &str = "cmd-alt-j";
#[cfg(not(target_os = "macos"))]
const COPY_JSONL: &str = "ctrl-alt-j";
#[cfg(target_os = "macos")]
const COPY_CSV: &str = "cmd-alt-s";
#[cfg(not(target_os = "macos"))]
const COPY_CSV: &str = "ctrl-alt-s";
#[cfg(target_os = "macos")]
const COPY_DOC_CURL: &str = "cmd-alt-u";
#[cfg(not(target_os = "macos"))]
const COPY_DOC_CURL: &str = "ctrl-alt-u";
#[cfg(target_os = "macos")]
const SAVE_MODE: &str = "cmd-shift-m";
#[cfg(not(target_os = "macos"))]
const SAVE_MODE: &str = "ctrl-shift-m";
#[cfg(target_os = "macos")]
const REVERT: &str = "cmd-shift-r";
#[cfg(not(target_os = "macos"))]
const REVERT: &str = "ctrl-shift-r";
#[cfg(target_os = "macos")]
const SELECT_ALL: &str = "cmd-a";
#[cfg(not(target_os = "macos"))]
const SELECT_ALL: &str = "ctrl-a";
#[cfg(target_os = "macos")]
const DELETE_QUERY: &str = "cmd-shift-k";
#[cfg(not(target_os = "macos"))]
const DELETE_QUERY: &str = "ctrl-shift-k";
#[cfg(target_os = "macos")]
const UPDATE_QUERY: &str = "cmd-shift-u";
#[cfg(not(target_os = "macos"))]
const UPDATE_QUERY: &str = "ctrl-shift-u";
#[cfg(target_os = "macos")]
const OPEN_SEARCH: &str = "cmd-shift-e";
#[cfg(not(target_os = "macos"))]
const OPEN_SEARCH: &str = "ctrl-shift-e";
#[cfg(target_os = "macos")]
const SHOW_HITS: &str = "cmd-3";
#[cfg(not(target_os = "macos"))]
const SHOW_HITS: &str = "ctrl-3";
#[cfg(target_os = "macos")]
const SHOW_RAW: &str = "cmd-4";
#[cfg(not(target_os = "macos"))]
const SHOW_RAW: &str = "ctrl-4";
#[cfg(target_os = "macos")]
const TOGGLE_INSTANT: &str = "cmd-alt-l";
#[cfg(not(target_os = "macos"))]
const TOGGLE_INSTANT: &str = "ctrl-alt-l";
#[cfg(target_os = "macos")]
const PREVIOUS_PAGE: &str = "cmd-alt-up";
#[cfg(not(target_os = "macos"))]
const PREVIOUS_PAGE: &str = "ctrl-alt-up";
#[cfg(target_os = "macos")]
const NEXT_PAGE: &str = "cmd-alt-down";
#[cfg(not(target_os = "macos"))]
const NEXT_PAGE: &str = "ctrl-alt-down";
#[cfg(target_os = "macos")]
const ADD_SEARCH: &str = "cmd-t";
#[cfg(not(target_os = "macos"))]
const ADD_SEARCH: &str = "ctrl-t";
#[cfg(target_os = "macos")]
const REMOVE_SEARCH: &str = "cmd-shift-w";
#[cfg(not(target_os = "macos"))]
const REMOVE_SEARCH: &str = "ctrl-shift-w";
#[cfg(target_os = "macos")]
const NEXT_SEARCH: &str = "cmd-alt-right";
#[cfg(not(target_os = "macos"))]
const NEXT_SEARCH: &str = "ctrl-alt-right";
#[cfg(target_os = "macos")]
const PREVIOUS_SEARCH: &str = "cmd-alt-left";
#[cfg(not(target_os = "macos"))]
const PREVIOUS_SEARCH: &str = "ctrl-alt-left";
#[cfg(target_os = "macos")]
const OPEN_HISTORY: &str = "cmd-shift-h";
#[cfg(not(target_os = "macos"))]
const OPEN_HISTORY: &str = "ctrl-shift-h";
#[cfg(target_os = "macos")]
const OPEN_SAVED: &str = "cmd-o";
#[cfg(not(target_os = "macos"))]
const OPEN_SAVED: &str = "ctrl-o";
#[cfg(target_os = "macos")]
const SAVE_QUERY: &str = "cmd-s";
#[cfg(not(target_os = "macos"))]
const SAVE_QUERY: &str = "ctrl-s";
#[cfg(target_os = "macos")]
const OPEN_PRESETS: &str = "cmd-alt-p";
#[cfg(not(target_os = "macos"))]
const OPEN_PRESETS: &str = "ctrl-alt-p";
#[cfg(all(feature = "frame-overlay", target_os = "macos"))]
const FRAME_OVERLAY: &str = "cmd-alt-f";
#[cfg(all(feature = "frame-overlay", not(target_os = "macos")))]
const FRAME_OVERLAY: &str = "ctrl-alt-f";
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
        // ⌘E edits a connection only in SIDEBAR_ENTRY, so the two never meet.
        KeyBinding::new(EDIT, EditSchema, Some(COLLECTIONS)),
        KeyBinding::new(EDIT, EditSchema, Some(COLLECTION)),
        // ⌘⌫ deletes a connection only in SIDEBAR_ENTRY, so the two never meet.
        KeyBinding::new(TRUNCATE, TruncateCollection, Some(COLLECTIONS)),
        KeyBinding::new(TRUNCATE, TruncateCollection, Some(COLLECTION)),
        KeyBinding::new(DELETE, DeleteCollection, Some(COLLECTIONS)),
        KeyBinding::new(DELETE, DeleteCollection, Some(COLLECTION)),
        // The same chords as the schema dialog, which is outside COLLECTION.
        KeyBinding::new(SHOW_FORM, ShowDocuments, Some(COLLECTION)),
        KeyBinding::new(SHOW_JSON, ShowSchema, Some(COLLECTION)),
        KeyBinding::new(SORT, SortByColumn, Some(DOCUMENTS)),
        KeyBinding::new(COLUMNS, ToggleColumns, Some(DOCUMENTS)),
        KeyBinding::new(PIN, TogglePinColumn, Some(DOCUMENTS)),
        KeyBinding::new(NARROWER, NarrowerColumn, Some(DOCUMENTS)),
        KeyBinding::new(WIDER, WiderColumn, Some(DOCUMENTS)),
        KeyBinding::new(FIND, FocusSearch, Some(DOCUMENTS)),
        KeyBinding::new(CLEAR_FILTER, ClearFilter, Some(DOCUMENTS)),
        KeyBinding::new(TOGGLE_INSPECTOR, ToggleInspector, Some(DOCUMENTS)),
        KeyBinding::new(GO_TO_ID, GoToId, Some(DOCUMENTS)),
        KeyBinding::new(COPY_ROW, CopyRowJson, Some(DOCUMENTS)),
        KeyBinding::new(COPY_JSONL, CopyJsonl, Some(DOCUMENTS)),
        KeyBinding::new(COPY_CSV, CopyCsv, Some(DOCUMENTS)),
        KeyBinding::new(COPY_DOC_CURL, CopyCurl, Some(DOCUMENTS)),
        // The same chord as New collection, which is bound only in COLLECTIONS.
        KeyBinding::new(NEW_COLLECTION, NewDocument, Some(DOCUMENTS)),
        KeyBinding::new(SAVE, ReviewDocument, Some(INSPECTOR)),
        KeyBinding::new(SAVE, ReviewDocument, Some(INSPECTOR_EDITOR)),
        KeyBinding::new(SAVE_MODE, ToggleSaveMode, Some(INSPECTOR)),
        KeyBinding::new(REVERT, RevertDocument, Some(INSPECTOR)),
        KeyBinding::new(SAVE, ApplyDocument, Some(DOCUMENT_REVIEW)),
        KeyBinding::new("enter", OpenDocument, Some(DOCUMENTS_TABLE)),
        // Only on the table: elsewhere ⌘C copies the selected text.
        KeyBinding::new(COPY_CELL, CopyCell, Some(DOCUMENTS_TABLE)),
        // Only on the table: in an input these are text and select all.
        KeyBinding::new("space", ToggleRow, Some(DOCUMENTS_TABLE)),
        KeyBinding::new("shift-space", ExtendRows, Some(DOCUMENTS_TABLE)),
        KeyBinding::new(SELECT_ALL, SelectLoaded, Some(DOCUMENTS_TABLE)),
        // Schema actions must not fire from the documents view, where they are not shown.
        KeyBinding::new(EDIT, NoAction, Some(DOCUMENTS)),
        KeyBinding::new(CLONE, NoAction, Some(DOCUMENTS)),
        KeyBinding::new(TRUNCATE, NoAction, Some(DOCUMENTS)),
        // Deeper than COLLECTION, so here ⌘⌫ never reaches the collection's delete.
        KeyBinding::new(DELETE, DeleteDocuments, Some(DOCUMENTS)),
        KeyBinding::new(DELETE_QUERY, DeleteByQuery, Some(DOCUMENTS)),
        KeyBinding::new(UPDATE_QUERY, UpdateByQuery, Some(DOCUMENTS)),
        KeyBinding::new(SAVE, BulkContinue, Some(BULK_DIALOG)),
        KeyBinding::new(SAVE, BulkContinue, Some(BULK_INPUT)),
        KeyBinding::new(SHOW_FORM, ShowForm, Some(SCHEMA_DIALOG)),
        KeyBinding::new(SHOW_JSON, ShowJson, Some(SCHEMA_DIALOG)),
        KeyBinding::new(SAVE, SubmitSchema, Some(SCHEMA_DIALOG)),
        // Deeper than the dialog, so in the last row ⌘↩ adds a field instead of creating.
        KeyBinding::new(SAVE, AddField, Some(SCHEMA_LAST_FIELD)),
        KeyBinding::new(OPEN_SEARCH, OpenSearchTab, None),
        KeyBinding::new(LOCK_AGAIN, LockAgain, Some(WORKBENCH)),
        KeyBinding::new(SAVE, RunSearch, Some(WORKBENCH)),
        KeyBinding::new(SAVE, RunSearch, Some(WORKBENCH_INPUT)),
        KeyBinding::new(SHOW_FORM, ShowForm, Some(WORKBENCH)),
        KeyBinding::new(SHOW_JSON, ShowJson, Some(WORKBENCH)),
        KeyBinding::new(SHOW_HITS, ShowHits, Some(WORKBENCH)),
        KeyBinding::new(SHOW_RAW, ShowRawResult, Some(WORKBENCH)),
        KeyBinding::new(TOGGLE_INSTANT, ToggleInstant, Some(WORKBENCH)),
        KeyBinding::new(FIND, FocusQuery, Some(WORKBENCH)),
        KeyBinding::new(PREVIOUS_PAGE, PreviousResultPage, Some(WORKBENCH)),
        KeyBinding::new(NEXT_PAGE, NextResultPage, Some(WORKBENCH)),
        KeyBinding::new(ADD_SEARCH, AddSearch, Some(WORKBENCH)),
        KeyBinding::new(REMOVE_SEARCH, RemoveSearch, Some(WORKBENCH)),
        KeyBinding::new(NEXT_SEARCH, NextSearch, Some(WORKBENCH)),
        KeyBinding::new(PREVIOUS_SEARCH, PreviousSearch, Some(WORKBENCH)),
        KeyBinding::new(SAVE_MODE, ToggleUnion, Some(WORKBENCH)),
        KeyBinding::new(OPEN_HISTORY, OpenHistory, Some(WORKBENCH)),
        KeyBinding::new(OPEN_SAVED, OpenSaved, Some(WORKBENCH)),
        KeyBinding::new(SAVE_QUERY, SaveQuery, Some(WORKBENCH)),
        KeyBinding::new(OPEN_PRESETS, OpenPresets, Some(WORKBENCH)),
        // Deeper than the input's own keys, so the filter keeps the focus.
        KeyBinding::new("up", SelectUp, Some(QUERY_LIST_INPUT)),
        KeyBinding::new("down", SelectDown, Some(QUERY_LIST_INPUT)),
        KeyBinding::new("enter", LoadQuery, Some(QUERY_LIST_INPUT)),
        KeyBinding::new(DELETE, DeleteSavedQuery, Some(QUERY_LIST_INPUT)),
        // Deeper than Run, so in the row ⌘↩ adds the condition.
        KeyBinding::new(SAVE, AddCondition, Some(CONDITION_ROW)),
        KeyBinding::new(SAVE, AddCondition, Some(CONDITION_INPUT)),
        // Only here: an input keeps its arrows.
        KeyBinding::new("up", SelectUp, Some(SEARCH_HITS)),
        KeyBinding::new("down", SelectDown, Some(SEARCH_HITS)),
        KeyBinding::new("alt-left", MoveChipLeft, Some(CHIP)),
        KeyBinding::new("alt-right", MoveChipRight, Some(CHIP)),
        KeyBinding::new("delete", RemoveChip, Some(CHIP)),
        KeyBinding::new("backspace", RemoveChip, Some(CHIP)),
    ]);
    #[cfg(feature = "frame-overlay")]
    cx.bind_keys([KeyBinding::new(
        FRAME_OVERLAY,
        ToggleFrameOverlay,
        Some(DOCUMENTS),
    )]);
    cx.on_action(|_: &Quit, cx| cx.quit());
}
