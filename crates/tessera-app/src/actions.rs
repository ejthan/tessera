// SPDX-License-Identifier: GPL-3.0-or-later

//! App-wide actions and their key bindings.

use gpui_kit::base::actions::{SelectDown, SelectUp};
use gpui_kit::component::dock::{ClosePanel, ToggleZoom};
use std::rc::Rc;

use gpui_kit::{Action, App, KeyBinding, KeyBindingContextPredicate, KeyContext, NoAction};
use rust_i18n::t;
use tessera_core::keymap::UserBinding;
use tessera_widgets::{BODY_VIEW, NextChunk, PreviousChunk};

gpui_kit::actions!(
    tessera,
    [
        NewConnection,
        Quit,
        ConnectSelected,
        DisconnectSelected,
        EditConnection,
        EditTransport,
        DeleteConnection,
        ImportProfiles,
        ExportProfiles,
        ToggleReopenLast,
        TestConnection,
        SaveConnection,
        ToggleKeyMask,
        ToggleRequestLog,
        CopyAsCurl,
        CopyKey,
        FinishReveal,
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
        OpenConsoleTab,
        StopWaiting,
        RemoveBodyFile,
        ShowResponseBody,
        ShowResponseHeaders,
        ShowFullBody,
        SendRequest,
        AddQueryRow,
        AttachFile,
        FavoriteRequest,
        RevealSecrets,
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
        DeleteSavedQuery,
        OpenExport,
        OpenPalette,
        OpenSettings,
        QuickSwitch,
        ToggleJobs,
        CancelJob,
        DismissJob,
        SaveReport,
        SaveFailedLines,
        ImportDocuments,
        PickImportFile,
        ImportSavedLines,
        EditImportSchema,
        PasteImport,
        StartTransfer,
        ExportDocuments,
        InferFromFile,
        OpenAliases,
        OpenSynonyms,
        OpenCurations,
        OpenStopwords,
        OpenPresetsTab,
        OpenKeys,
        OpenAnalytics,
        OpenStemming,
        OpenAi,
        OpenOperations,
        OpenBackup,
        RunSnapshot,
        RunCompact,
        RunClearCache,
        RunVote,
        FlushAnalytics,
        SendTestEvent,
        OpenTopInSearch,
        NewResource,
        EditResource,
        DeleteResource,
        ReloadResource,
        FocusResourceFilter,
        SaveResource,
        LeavePreset,
        NewSetItem,
        DeleteSetItem,
        ShowItems,
        ShowSetJson,
        ShowAttachment,
        ImportResource,
        ExportResource,
        DetachMissing,
        CreateMissingSet,
        ShowTest,
        RunTest,
        NewConversation,
        MoveRowUp,
        MoveRowDown,
        NextControl,
        PreviousControl,
        ReindexAlias,
        NextStep,
        StepBack
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
/// Key context of the request log panel.
pub const REQUEST_LOG: &str = "RequestLog";
/// Key context of the jobs panel.
pub const JOBS: &str = "Jobs";
/// Key context of the import dialog.
pub const IMPORT_DIALOG: &str = "DocumentImport";
/// An input in that dialog, which has its own ⌘↩.
const IMPORT_INPUT: &str = "DocumentImport > Input";
/// Key context of the export dialog.
pub const EXPORT_DIALOG: &str = "DocumentExport";
/// An input in that dialog, which has its own ⌘↩.
const EXPORT_INPUT: &str = "DocumentExport > Input";
/// Key context of the raw console.
pub const CONSOLE: &str = "Console";
/// Key context of an input in the console, where ⌘↩ / Ctrl+↩ still sends.
const CONSOLE_INPUT: &str = "Console > Input";
/// Key context of a resource tab.
pub const RESOURCES: &str = "Resources";
/// An input in a resource tab, which has its own ⌘↩.
const RESOURCES_INPUT: &str = "Resources > Input";
/// Key context of the scoped key panel.
pub const SCOPED_KEYS: &str = "ScopedKeys";
/// Key context of the dialog that shows a new key.
pub const KEY_REVEAL: &str = "KeyReveal";
/// Key context of the stemming import dialog.
pub const STEMMING_DIALOG: &str = "StemmingDialog";
/// An input in that dialog, which has its own ⌘↩.
const STEMMING_INPUT: &str = "StemmingDialog > Input";
/// Key context of an analytics tab.
pub const ANALYTICS: &str = "Analytics";
/// Key context of the alias dialog.
pub const ALIAS_DIALOG: &str = "AliasDialog";
/// An input in that dialog, which has its own ⌘↩.
const ALIAS_INPUT: &str = "AliasDialog > Input";
/// Key context of a reindex tab.
pub const REINDEX: &str = "Reindex";
/// An input in that tab, which has its own ⌘↩.
const REINDEX_INPUT: &str = "Reindex > Input";
/// Key context of the items of a set, where New and Delete mean an item.
pub const SET_ITEMS: &str = "SetItems";
/// Key context of the item dialog.
pub const SET_ITEM_DIALOG: &str = "SetItemDialog";
/// An input in that dialog, which has its own ⌘↩.
const SET_ITEM_INPUT: &str = "SetItemDialog > Input";
/// Key context of the import and export dialogs of a set.
pub const SET_IMPORT_DIALOG: &str = "SetImportDialog";
/// An input in that dialog, which has its own ⌘↩.
const SET_IMPORT_INPUT: &str = "SetImportDialog > Input";
/// Key context of a raw JSON block in a form.
pub const RAW_BLOCK: &str = "RawBlock";
/// The editor of that block: its Tab moves on, or a form would end there.
const RAW_BLOCK_INPUT: &str = "RawBlock > Input";
/// Key context of a row of pinned documents.
pub const INCLUDE_ROW: &str = "IncludeRow";
/// An input in that row, which has its own ⌘↩.
const INCLUDE_ROW_INPUT: &str = "IncludeRow > Input";
/// Key context of the test panel of a set.
pub const TEST_PANEL: &str = "TestPanel";
/// An input in that panel, which has its own ⌘↩.
const TEST_PANEL_INPUT: &str = "TestPanel > Input";
/// Key context of a list in a toolbar.
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
const STOP_WAITING: &str = "alt-cmd-.";
#[cfg(not(target_os = "macos"))]
const STOP_WAITING: &str = "ctrl-alt-.";
#[cfg(target_os = "macos")]
const REMOVE_FILE: &str = "alt-cmd-o";
#[cfg(not(target_os = "macos"))]
const REMOVE_FILE: &str = "ctrl-alt-o";
#[cfg(target_os = "macos")]
const PREVIOUS_CHUNK: &str = "alt-cmd-[";
#[cfg(not(target_os = "macos"))]
const PREVIOUS_CHUNK: &str = "ctrl-alt-[";
#[cfg(target_os = "macos")]
const NEXT_CHUNK: &str = "alt-cmd-]";
#[cfg(not(target_os = "macos"))]
const NEXT_CHUNK: &str = "ctrl-alt-]";
#[cfg(target_os = "macos")]
const SHOW_FULL_BODY: &str = "cmd-shift-b";
#[cfg(not(target_os = "macos"))]
const SHOW_FULL_BODY: &str = "ctrl-shift-b";
#[cfg(target_os = "macos")]
const OPEN_CONSOLE: &str = "cmd-shift-y";
#[cfg(not(target_os = "macos"))]
const OPEN_CONSOLE: &str = "ctrl-shift-y";
#[cfg(target_os = "macos")]
const OPEN_RESOURCES: [&str; 10] = [
    "alt-cmd-1",
    "alt-cmd-2",
    "alt-cmd-3",
    "alt-cmd-4",
    "alt-cmd-5",
    "alt-cmd-6",
    "alt-cmd-7",
    "alt-cmd-8",
    "alt-cmd-9",
    "alt-cmd-0",
];
#[cfg(not(target_os = "macos"))]
const OPEN_RESOURCES: [&str; 10] = [
    "ctrl-alt-1",
    "ctrl-alt-2",
    "ctrl-alt-3",
    "ctrl-alt-4",
    "ctrl-alt-5",
    "ctrl-alt-6",
    "ctrl-alt-7",
    "ctrl-alt-8",
    "ctrl-alt-9",
    "ctrl-alt-0",
];
#[cfg(target_os = "macos")]
const OPEN_BACKUP: &str = "alt-cmd-b";
#[cfg(not(target_os = "macos"))]
const OPEN_BACKUP: &str = "ctrl-alt-b";
#[cfg(target_os = "macos")]
const OPEN_TOP: &str = "cmd-alt-g";
#[cfg(not(target_os = "macos"))]
const OPEN_TOP: &str = "ctrl-alt-g";
#[cfg(target_os = "macos")]
const ADD_QUERY_ROW: &str = "cmd-shift-a";
#[cfg(not(target_os = "macos"))]
const ADD_QUERY_ROW: &str = "ctrl-shift-a";
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
const LEAVE_PRESET: &str = "cmd-shift-g";
#[cfg(not(target_os = "macos"))]
const LEAVE_PRESET: &str = "ctrl-shift-g";
#[cfg(target_os = "macos")]
const SAVE_QUERY: &str = "cmd-s";
#[cfg(not(target_os = "macos"))]
const SAVE_QUERY: &str = "ctrl-s";
#[cfg(target_os = "macos")]
const OPEN_PRESETS: &str = "cmd-alt-p";
#[cfg(not(target_os = "macos"))]
const OPEN_PRESETS: &str = "ctrl-alt-p";
#[cfg(target_os = "macos")]
const OPEN_EXPORT: &str = "cmd-alt-e";
#[cfg(not(target_os = "macos"))]
const OPEN_EXPORT: &str = "ctrl-alt-e";
#[cfg(target_os = "macos")]
const OPEN_PALETTE: &str = "cmd-k";
#[cfg(not(target_os = "macos"))]
const OPEN_PALETTE: &str = "ctrl-k";
#[cfg(target_os = "macos")]
const OPEN_SETTINGS: &str = "cmd-,";
#[cfg(not(target_os = "macos"))]
const OPEN_SETTINGS: &str = "ctrl-,";
#[cfg(target_os = "macos")]
const QUICK_SWITCH: &str = "cmd-p";
#[cfg(not(target_os = "macos"))]
const QUICK_SWITCH: &str = "ctrl-p";
#[cfg(target_os = "macos")]
const IMPORT_DOCUMENTS: &str = "cmd-shift-i";
#[cfg(not(target_os = "macos"))]
const IMPORT_DOCUMENTS: &str = "ctrl-shift-i";
#[cfg(target_os = "macos")]
const EXPORT_DOCUMENTS: &str = "cmd-shift-x";
#[cfg(not(target_os = "macos"))]
const EXPORT_DOCUMENTS: &str = "ctrl-shift-x";
#[cfg(target_os = "macos")]
const PASTE_IMPORT: &str = "cmd-shift-v";
#[cfg(not(target_os = "macos"))]
const PASTE_IMPORT: &str = "ctrl-shift-v";
#[cfg(target_os = "macos")]
const TOGGLE_JOBS: &str = "cmd-shift-j";
#[cfg(not(target_os = "macos"))]
const TOGGLE_JOBS: &str = "ctrl-shift-j";
#[cfg(target_os = "macos")]
const CANCEL_JOB: &str = "cmd-.";
#[cfg(not(target_os = "macos"))]
const CANCEL_JOB: &str = "ctrl-.";
#[cfg(all(feature = "frame-overlay", target_os = "macos"))]
const FRAME_OVERLAY: &str = "cmd-alt-f";
#[cfg(all(feature = "frame-overlay", not(target_os = "macos")))]
const FRAME_OVERLAY: &str = "ctrl-alt-f";
#[cfg(target_os = "macos")]
const EDIT_TRANSPORT: &str = "cmd-alt-x";
#[cfg(not(target_os = "macos"))]
const EDIT_TRANSPORT: &str = "ctrl-alt-x";
#[cfg(target_os = "macos")]
const QUIT: &str = "cmd-q";
#[cfg(not(target_os = "macos"))]
const QUIT: &str = "ctrl-q";

macro_rules! action_labels {
    ($($name:ident),* $(,)?) => {
        &[$((concat!("tessera::", stringify!($name)), concat!("palette.action.", stringify!($name)))),*]
    };
}

/// Each action the palette lists, with the key of its label.
const ACTION_LABELS: &[(&str, &str)] = action_labels![
    NewConnection,
    Quit,
    DisconnectSelected,
    EditConnection,
    EditTransport,
    DeleteConnection,
    ImportProfiles,
    ExportProfiles,
    ToggleReopenLast,
    OpenSettings,
    ToggleRequestLog,
    CopyAsCurl,
    CopyKey,
    FinishReveal,
    TogglePolling,
    Refresh,
    NewCollection,
    ToggleFavorite,
    FocusSearch,
    LockAgain,
    CloneCollection,
    EditSchema,
    ShowForm,
    ShowJson,
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
    SelectLoaded,
    DeleteDocuments,
    DeleteByQuery,
    UpdateByQuery,
    OpenSearchTab,
    OpenConsoleTab,
    OpenAliases,
    OpenSynonyms,
    OpenCurations,
    OpenStopwords,
    OpenPresetsTab,
    OpenKeys,
    OpenAnalytics,
    OpenStemming,
    OpenAi,
    OpenOperations,
    OpenBackup,
    RunSnapshot,
    RunCompact,
    RunClearCache,
    RunVote,
    FlushAnalytics,
    SendTestEvent,
    OpenTopInSearch,
    NewResource,
    EditResource,
    DeleteResource,
    ReloadResource,
    FocusResourceFilter,
    SaveResource,
    LeavePreset,
    NewSetItem,
    DeleteSetItem,
    ShowItems,
    ShowSetJson,
    ShowAttachment,
    ImportResource,
    ExportResource,
    DetachMissing,
    CreateMissingSet,
    ShowTest,
    RunTest,
    NewConversation,
    StopWaiting,
    RemoveBodyFile,
    ShowResponseBody,
    ShowResponseHeaders,
    ShowFullBody,
    SendRequest,
    AddQueryRow,
    AttachFile,
    FavoriteRequest,
    RevealSecrets,
    RunSearch,
    ShowHits,
    ShowRawResult,
    ToggleInstant,
    FocusQuery,
    NextResultPage,
    PreviousResultPage,
    AddSearch,
    RemoveSearch,
    NextSearch,
    PreviousSearch,
    ToggleUnion,
    OpenHistory,
    OpenSaved,
    SaveQuery,
    OpenPresets,
    OpenExport,
    QuickSwitch,
    ToggleJobs,
    CancelJob,
    DismissJob,
    SaveReport,
    SaveFailedLines,
    ImportDocuments,
    ExportDocuments,
    ReindexAlias,
];

/// Actions the palette never lists: steps that mean nothing outside their control, and
/// actions of dialogs, over which the palette does not open.
pub const PALETTE_HIDDEN: &[&str] = &[
    "tessera::OpenPalette",
    "tessera::ConnectSelected",
    "tessera::TestConnection",
    "tessera::SaveConnection",
    "tessera::ToggleKeyMask",
    "tessera::OpenCollectionRow",
    "tessera::OpenLink",
    "tessera::AddField",
    "tessera::SubmitSchema",
    "tessera::OpenDocument",
    "tessera::ApplyDocument",
    "tessera::ToggleRow",
    "tessera::ExtendRows",
    "tessera::BulkContinue",
    "tessera::AddCondition",
    "tessera::MoveChipLeft",
    "tessera::MoveChipRight",
    "tessera::RemoveChip",
    "tessera::LoadQuery",
    "tessera::DeleteSavedQuery",
    "tessera::ToggleFrameOverlay",
    "tessera::PickImportFile",
    "tessera::ImportSavedLines",
    "tessera::EditImportSchema",
    "tessera::PasteImport",
    "tessera::StartTransfer",
    "tessera::InferFromFile",
    "tessera::NextStep",
    "tessera::StepBack",
    "tessera::MoveRowUp",
    "tessera::MoveRowDown",
    "tessera::NextControl",
    "tessera::PreviousControl",
];

/// The locale key of the label of action `name`, when the palette lists it.
pub fn action_label(name: &str) -> Option<&'static str> {
    ACTION_LABELS
        .iter()
        .find(|(action, _)| *action == name)
        .map(|(_, key)| *key)
}

/// `context` naming the actions whose control is disabled now, for the palette to leave out.
/// Their handlers stay registered: without one a chord would fall through to a shallower
/// binding, or to an input's own key.
pub fn gated(context: &str, gates: &[(bool, &dyn Action)]) -> KeyContext {
    let mut key = KeyContext::try_from(context).unwrap_or_default();
    for (_, action) in gates.iter().filter(|(disabled, _)| *disabled) {
        key.add(action.name());
    }
    key
}

/// Applies the user's bindings after [`bind_keys`], so they win. A bad entry is skipped and
/// named in the returned lines; the others apply.
pub fn apply_keymap(bindings: &[UserBinding], cx: &mut App) -> Vec<String> {
    let mut errors = Vec::new();
    for entry in bindings {
        match user_binding(entry, cx) {
            Ok(binding) => cx.bind_keys([binding]),
            Err(error) => errors.push(error),
        }
    }
    errors
}

fn user_binding(entry: &UserBinding, cx: &App) -> Result<KeyBinding, String> {
    let key = entry.key.as_str();
    let action = match &entry.action {
        None => Box::new(NoAction),
        Some(name) => cx.build_action(name, None).map_err(|e| {
            let detail = e.to_string();
            t!(
                "keymap.error.action",
                key = key,
                action = name,
                detail = detail
            )
            .to_string()
        })?,
    };
    let context = match &entry.context {
        None => None,
        Some(text) => Some(Rc::new(KeyBindingContextPredicate::parse(text).map_err(
            |e| {
                let detail = e.to_string();
                t!(
                    "keymap.error.context",
                    key = key,
                    context = text,
                    detail = detail
                )
                .to_string()
            },
        )?)),
    };
    let binding = KeyBinding::load(key, action, context, false, None, cx).map_err(|e| {
        let detail = e.to_string();
        t!("keymap.error.key", key = key, detail = detail).to_string()
    })?;
    let Some(first) = binding.keystrokes().first().map(|k| k.inner()) else {
        return Err(t!("keymap.error.empty").to_string());
    };
    // Esc is how a dialog closes, and how a confirmation is refused: no entry may take it,
    // nor start a sequence with it, which would hold it back.
    if first.key == "escape" && !first.modifiers.modified() {
        return Err(t!("keymap.error.reserved", key = key).to_string());
    }
    Ok(binding)
}

/// Registers global key bindings and app-level action handlers.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new(NEW_CONNECTION, NewConnection, None),
        KeyBinding::new(QUIT, Quit, None),
        KeyBinding::new(OPEN_PALETTE, OpenPalette, None),
        KeyBinding::new(OPEN_SETTINGS, OpenSettings, None),
        KeyBinding::new(QUICK_SWITCH, QuickSwitch, None),
        KeyBinding::new(TOGGLE_LOG, ToggleRequestLog, None),
        KeyBinding::new(TOGGLE_JOBS, ToggleJobs, None),
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
        KeyBinding::new(EDIT_TRANSPORT, EditTransport, Some(OVERVIEW)),
        KeyBinding::new(EDIT_TRANSPORT, EditTransport, Some(COLLECTIONS)),
        KeyBinding::new(EDIT_TRANSPORT, EditTransport, Some(COLLECTION)),
        KeyBinding::new(EDIT_TRANSPORT, EditTransport, Some(RESOURCES)),
        KeyBinding::new(EDIT_TRANSPORT, EditTransport, Some(REINDEX)),
        KeyBinding::new(EDIT_TRANSPORT, EditTransport, Some(CONSOLE)),
        KeyBinding::new(EDIT_TRANSPORT, EditTransport, Some(WORKBENCH)),
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
        KeyBinding::new(OPEN_CONSOLE, OpenConsoleTab, None),
        KeyBinding::new(OPEN_RESOURCES[0], OpenAliases, None),
        KeyBinding::new(OPEN_RESOURCES[1], OpenSynonyms, None),
        KeyBinding::new(OPEN_RESOURCES[2], OpenCurations, None),
        KeyBinding::new(OPEN_RESOURCES[3], OpenStopwords, None),
        KeyBinding::new(OPEN_RESOURCES[4], OpenPresetsTab, None),
        KeyBinding::new(OPEN_RESOURCES[5], OpenKeys, None),
        KeyBinding::new(OPEN_RESOURCES[6], OpenAnalytics, None),
        KeyBinding::new(OPEN_RESOURCES[7], OpenStemming, None),
        KeyBinding::new(OPEN_RESOURCES[8], OpenAi, None),
        KeyBinding::new(OPEN_RESOURCES[9], OpenOperations, None),
        KeyBinding::new(OPEN_BACKUP, OpenBackup, None),
        KeyBinding::new(OPEN_SAVED, PickImportFile, Some(RESOURCES)),
        KeyBinding::new(STOP_WAITING, StopWaiting, Some(RESOURCES)),
        KeyBinding::new(SAVE, SaveResource, Some(STEMMING_DIALOG)),
        KeyBinding::new(SAVE, SaveResource, Some(STEMMING_INPUT)),
        KeyBinding::new(OPEN_SAVED, PickImportFile, Some(STEMMING_DIALOG)),
        KeyBinding::new(UPDATE_QUERY, FlushAnalytics, Some(ANALYTICS)),
        KeyBinding::new(TEST, SendTestEvent, Some(ANALYTICS)),
        KeyBinding::new(OPEN_TOP, OpenTopInSearch, Some(ANALYTICS)),
        KeyBinding::new(LOCK_AGAIN, LockAgain, Some(RESOURCES)),
        KeyBinding::new(REVERT, ReindexAlias, Some(RESOURCES)),
        KeyBinding::new(LOCK_AGAIN, LockAgain, Some(REINDEX)),
        KeyBinding::new(SAVE, NextStep, Some(REINDEX)),
        KeyBinding::new(SAVE, NextStep, Some(REINDEX_INPUT)),
        KeyBinding::new(REVERT, StepBack, Some(REINDEX)),
        KeyBinding::new(EDIT, EditResource, Some(REINDEX)),
        KeyBinding::new(DELETE, DeleteResource, Some(REINDEX)),
        KeyBinding::new(REFRESH, ReloadResource, Some(REINDEX)),
        KeyBinding::new(CANCEL_JOB, CancelJob, Some(REINDEX)),
        KeyBinding::new(STOP_WAITING, StopWaiting, Some(REINDEX)),
        KeyBinding::new(SAVE_QUERY, SaveReport, Some(REINDEX)),
        KeyBinding::new(OPEN_SAVED, PickImportFile, Some(REINDEX)),
        KeyBinding::new(REMOVE_FILE, RemoveBodyFile, Some(REINDEX)),
        KeyBinding::new(NEW_COLLECTION, NewResource, Some(RESOURCES)),
        KeyBinding::new(EDIT, EditResource, Some(RESOURCES)),
        KeyBinding::new(DELETE, DeleteResource, Some(RESOURCES)),
        KeyBinding::new(REFRESH, ReloadResource, Some(RESOURCES)),
        KeyBinding::new(FIND, FocusResourceFilter, Some(RESOURCES)),
        KeyBinding::new(SAVE, SaveResource, Some(RESOURCES)),
        KeyBinding::new(SAVE, SaveResource, Some(RESOURCES_INPUT)),
        KeyBinding::new(SAVE, SaveResource, Some(ALIAS_DIALOG)),
        KeyBinding::new(SAVE, SaveResource, Some(ALIAS_INPUT)),
        KeyBinding::new(SAVE, SaveResource, Some(SET_ITEM_DIALOG)),
        KeyBinding::new(SAVE, SaveResource, Some(SET_ITEM_INPUT)),
        KeyBinding::new(SAVE, SaveResource, Some(INCLUDE_ROW_INPUT)),
        KeyBinding::new("alt-up", MoveRowUp, Some(INCLUDE_ROW)),
        KeyBinding::new("alt-down", MoveRowDown, Some(INCLUDE_ROW)),
        KeyBinding::new("tab", NextControl, Some(RAW_BLOCK_INPUT)),
        KeyBinding::new("shift-tab", PreviousControl, Some(RAW_BLOCK_INPUT)),
        KeyBinding::new(SAVE, SaveResource, Some(RAW_BLOCK_INPUT)),
        KeyBinding::new(SHOW_FORM, ShowForm, Some(SET_ITEM_DIALOG)),
        KeyBinding::new(SHOW_JSON, ShowJson, Some(SET_ITEM_DIALOG)),
        KeyBinding::new(FIND, FocusQuery, Some(SET_ITEM_DIALOG)),
        KeyBinding::new(NEW_COLLECTION, NewSetItem, Some(SET_ITEMS)),
        KeyBinding::new(DELETE, DeleteSetItem, Some(SET_ITEMS)),
        KeyBinding::new(SHOW_FORM, ShowItems, Some(RESOURCES)),
        KeyBinding::new(SHOW_JSON, ShowSetJson, Some(RESOURCES)),
        // Not COPY_CURL, which stays Copy as curl here.
        KeyBinding::new(COPY_ROW, CopyKey, Some(SCOPED_KEYS)),
        KeyBinding::new(COPY_ROW, CopyKey, Some(KEY_REVEAL)),
        KeyBinding::new(SAVE, FinishReveal, Some(KEY_REVEAL)),
        KeyBinding::new(SHOW_FORM, ShowDocuments, Some(RESOURCES)),
        KeyBinding::new(SHOW_JSON, ShowSchema, Some(RESOURCES)),
        KeyBinding::new(SHOW_HITS, ShowAttachment, Some(RESOURCES)),
        KeyBinding::new(SHOW_RAW, ShowTest, Some(RESOURCES)),
        KeyBinding::new(SHOW_FORM, RunSnapshot, Some(RESOURCES)),
        KeyBinding::new(SHOW_JSON, RunCompact, Some(RESOURCES)),
        KeyBinding::new(SHOW_HITS, RunClearCache, Some(RESOURCES)),
        KeyBinding::new(SHOW_RAW, RunVote, Some(RESOURCES)),
        KeyBinding::new(SAVE, RunTest, Some(TEST_PANEL)),
        KeyBinding::new(ADD_QUERY_ROW, NewConversation, Some(TEST_PANEL)),
        KeyBinding::new(SAVE, RunTest, Some(TEST_PANEL_INPUT)),
        KeyBinding::new(FIND, FocusQuery, Some(TEST_PANEL)),
        KeyBinding::new(IMPORT_DOCUMENTS, ImportResource, Some(RESOURCES)),
        KeyBinding::new(EXPORT_DOCUMENTS, ExportResource, Some(RESOURCES)),
        KeyBinding::new(DELETE_QUERY, DetachMissing, Some(RESOURCES)),
        KeyBinding::new(SAVE_MODE, CreateMissingSet, Some(RESOURCES)),
        KeyBinding::new(SAVE, StartTransfer, Some(SET_IMPORT_DIALOG)),
        KeyBinding::new(SAVE, StartTransfer, Some(SET_IMPORT_INPUT)),
        KeyBinding::new(SHOW_FULL_BODY, ShowFullBody, Some(REQUEST_LOG)),
        KeyBinding::new(LOCK_AGAIN, LockAgain, Some(CONSOLE)),
        KeyBinding::new(SAVE, SendRequest, Some(CONSOLE)),
        KeyBinding::new(SAVE, SendRequest, Some(CONSOLE_INPUT)),
        KeyBinding::new(ADD_QUERY_ROW, AddQueryRow, Some(CONSOLE)),
        KeyBinding::new(STOP_WAITING, StopWaiting, Some(CONSOLE)),
        KeyBinding::new(REMOVE_FILE, RemoveBodyFile, Some(CONSOLE)),
        KeyBinding::new(SHOW_HITS, ShowResponseBody, Some(CONSOLE)),
        KeyBinding::new(SHOW_RAW, ShowResponseHeaders, Some(CONSOLE)),
        KeyBinding::new(PREVIOUS_CHUNK, PreviousChunk, Some(BODY_VIEW)),
        KeyBinding::new(NEXT_CHUNK, NextChunk, Some(BODY_VIEW)),
        KeyBinding::new(PREVIOUS_CHUNK, PreviousChunk, Some(CONSOLE)),
        KeyBinding::new(NEXT_CHUNK, NextChunk, Some(CONSOLE)),
        KeyBinding::new(PREVIOUS_CHUNK, PreviousChunk, Some(WORKBENCH)),
        KeyBinding::new(NEXT_CHUNK, NextChunk, Some(WORKBENCH)),
        KeyBinding::new(OPEN_SAVED, AttachFile, Some(CONSOLE)),
        KeyBinding::new(TOGGLE_FAVORITE, FavoriteRequest, Some(CONSOLE)),
        KeyBinding::new(TOGGLE_KEY, RevealSecrets, Some(CONSOLE)),
        KeyBinding::new(OPEN_HISTORY, OpenHistory, Some(CONSOLE)),
        KeyBinding::new(COPY_DOC_CURL, CopyCurl, Some(CONSOLE)),
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
        KeyBinding::new(LEAVE_PRESET, LeavePreset, Some(WORKBENCH)),
        KeyBinding::new(OPEN_PRESETS, OpenPresets, Some(WORKBENCH)),
        KeyBinding::new(OPEN_EXPORT, OpenExport, Some(WORKBENCH)),
        KeyBinding::new(COPY_DOC_CURL, CopyCurl, Some(WORKBENCH)),
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
        KeyBinding::new(IMPORT_DOCUMENTS, ImportDocuments, Some(COLLECTION)),
        KeyBinding::new(IMPORT_DOCUMENTS, ImportDocuments, Some(COLLECTIONS)),
        KeyBinding::new(OPEN_SAVED, PickImportFile, Some(IMPORT_DIALOG)),
        KeyBinding::new(PASTE_IMPORT, PasteImport, Some(IMPORT_DIALOG)),
        KeyBinding::new(EDIT, EditImportSchema, Some(IMPORT_DIALOG)),
        KeyBinding::new(SAVE, StartTransfer, Some(IMPORT_DIALOG)),
        KeyBinding::new(SAVE, StartTransfer, Some(IMPORT_INPUT)),
        KeyBinding::new(EXPORT_DOCUMENTS, ExportDocuments, Some(COLLECTION)),
        KeyBinding::new(OPEN_SAVED, InferFromFile, Some(SCHEMA_DIALOG)),
        KeyBinding::new(SAVE, StartTransfer, Some(EXPORT_DIALOG)),
        KeyBinding::new(SAVE, StartTransfer, Some(EXPORT_INPUT)),
        KeyBinding::new(CANCEL_JOB, CancelJob, Some(JOBS)),
        KeyBinding::new(STOP_WAITING, StopWaiting, Some(JOBS)),
        KeyBinding::new(DELETE, DismissJob, Some(JOBS)),
        KeyBinding::new(SAVE_QUERY, SaveReport, Some(JOBS)),
        KeyBinding::new(SORT, SaveFailedLines, Some(JOBS)),
        KeyBinding::new(OPEN_SAVED, ImportSavedLines, Some(JOBS)),
        KeyBinding::new(IMPORT_DOCUMENTS, ImportDocuments, Some(JOBS)),
    ]);
    #[cfg(feature = "frame-overlay")]
    cx.bind_keys([KeyBinding::new(
        FRAME_OVERLAY,
        ToggleFrameOverlay,
        Some(DOCUMENTS),
    )]);
    cx.on_action(|_: &Quit, cx| cx.quit());
}
