# UX

Mockups: [Tessera UX canvas](https://claude.ai/artifact/DucJnCzM4SMUhUo5UvVk1d). They show the M1 target; each milestone builds the part it needs.

## Screens on the canvas

1. **First run.** Welcome screen with "Add connection", "Import from typesense-dashboard" and "Run Typesense locally" (shows a Docker command with a Copy button, never runs it).
2. **Connection dialog.** Name, color, environment (local, dev, staging, prod), nodes, API key stored in the keychain, read-only switch, collapsed advanced options, inline connection test result.
3. **Overview.** Cluster nodes with leader and unreachable states, key figures, live metrics for 15 minutes, API stats, and the request log in the bottom panel.
4. **Production safety.** Red band and window frame for prod, "writes unlocked" banner with "Lock again", delete-collection dialog that needs the name typed, shows affected aliases and keeps Cancel focused.
5. **Document browser (dark).** `filter_by` bar with facet value autocompletion, table with a pinned `id` column, inspector with schema validation and "Review and save".

## Rules

1. A connection's environment is visible everywhere: title bar, tabs, sidebar, and for prod the window frame.
2. Errors show where they happen, inline and in full. Toasts are only for background events.
3. Default focus is never on a destructive action. High danger means typing the name, and the dialog states the concrete consequences (counts, affected aliases).
4. No dead controls: navigation entries, panels and buttons appear in the milestone that makes them work.
5. Every action is reachable by keyboard, and its shortcut is shown on the control.
