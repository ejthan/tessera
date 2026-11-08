# Backup and restore

The Backup row in the sidebar, ⌥⌘B / Ctrl+Alt+B, opens one tab with both.

## Backup

Pick collections, and a folder or a `.tar.zst`. The tab says what a backup holds and leaves out, and warns about unstored fields, left-out keys, dictionaries and references outside the selection. Start (⌘↩ / Ctrl+Enter) asks where to and refuses a name that exists. It needs no unlock.

A backup holds schemas, documents, attached sets, stopwords and aliases, in plain text and private to your user. It holds no provider key: credentials in a schema are blanked and counted.

The run is a job. It goes on with the tab closed and stops like an import.

## Restore

⌘2 / Ctrl+2 in the Backup tab, then Choose… (⌘O / Ctrl+O) with an archive or a folder. Tessera unpacks it, checks every file, and compares it with the server. Each stage has Stop, and a failure shows in full under the button.

One row per collection, set, stopword set and alias says **New**, **Same**, **Differs** or **Blocked**, with the reason.

- A restore never writes to an existing collection.
- Replace what differs is off by default.
- A collection whose provider keys were left out stays blocked. The row says which two files to use by hand.
- Start asks with the counts, and asks for the profile's name on prod or when something is replaced. A read-only connection unlocks first.

If a run stops, the tab shows each row's result and what to delete before a retry.
