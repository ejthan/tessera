# Collections and schema

## The list

The Collections row opens a table of the connection's collections with document and field counts, creation time, aliases and sets. Click a header to sort.

| Action | Shortcut |
|---|---|
| Search names and aliases | ⌘F / Ctrl+F |
| Open the selected collection | Enter |
| Favorite, shown first and in the sidebar | ⌘D / Ctrl+D |
| Reload | ⌘R / Ctrl+R |
| New collection | ⇧⌘N / Ctrl+Shift+N |
| Clone | ⇧⌘D / Ctrl+Shift+D |

A clone blanks embedding credentials.

## Create

The schema dialog has a form (⌘1 / Ctrl+1) and JSON (⌘2 / Ctrl+2), and the two stay in sync. ⌘↩ / Ctrl+Enter creates the collection, and in the last field row adds a field. Errors show in the dialog.

Infer from file (⌘O / Ctrl+O) proposes the fields from the first 1,000 rows of a file, with notes where that is a guess. It never picks `geopoint` by itself, so choose Use geopoint where a pair of numbers is one.

On a `float[]` field, Auto-embed sets the source fields and the model. A key you type for a remote model shows as `••••` in the JSON tab and the review. A model named `ts/…` is built in and is downloaded by the server at first use.

## Change a schema

Edit schema (⌘E / Ctrl+E) shows a review before anything is sent: added, dropped and changed fields, what follows from them, and the exact request body. While the change applies, the view shows its progress. Stop waiting leaves it running on the server.

## Truncate and delete

| Action | Shortcut |
|---|---|
| Truncate, deletes all documents and keeps the schema | ⇧⌘⌫ / Ctrl+Shift+Backspace |
| Delete, also lists the aliases left dangling | ⌘⌫ / Ctrl+Backspace |

Both show the current document count and need the collection's name typed.
