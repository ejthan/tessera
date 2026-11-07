# Import and export

Both run as jobs: they keep going when their dialog or tab closes.

## Import

⇧⌘I / Ctrl+Shift+I or Import, in the collections list or a collection's documents. Choose a file (⌘O / Ctrl+O), drop one on the dialog, or paste the clipboard up to 16 MiB (⇧⌘V / Ctrl+Shift+V).

Tessera detects JSON Lines, a JSON array or CSV. The first 20 rows show checked against the schema, with an estimate of the documents. Choose the action, dirty values and batch size, and press ⌘↩ / Ctrl+Enter.

- `create` asks only on prod. `upsert`, `update` and `emplace` always ask, and need the collection's name typed on prod or above 1,000 estimated documents.
- New collection… proposes a name and a schema from the file. The collection is created when the import starts, so cancelling leaves the server as it was.
- A read-only connection asks to unlock first.

## Export

⇧⌘X / Ctrl+Shift+X or Export in a collection's documents. Choose the whole collection, the applied filter or the ticked rows, which fields, and JSON Lines, a JSON array or CSV. Start asks where to save. An export only reads, so it needs no unlock.

A stopped or failed export leaves an existing file as it was. A spreadsheet may run a CSV cell starting with `=` as a formula, because cells stay verbatim.

## Jobs

⇧⌘J / Ctrl+Shift+J opens the Jobs tab. A row shows the connection, a bar when the total is known, documents, rate, time left and errors.

| Action | Shortcut |
|---|---|
| Stop after the current batch | ⌘. / Ctrl+. |
| Stop waiting for that batch | ⌥⌘. / Ctrl+Alt+. |
| Save the report | ⌘S / Ctrl+S |
| Save the failed lines | ⇧⌘S / Ctrl+Shift+S |
| Import a saved file of failed lines | ⌘O / Ctrl+O |
| Dismiss a job, which asks first | ⌘⌫ / Ctrl+Backspace |

A finished import shows imported, failed and not confirmed. A document ends up not confirmed when its batch ended in a server error or a broken connection, so it may or may not be stored.

A disconnect stops a write on an unlocked read-only connection at its next batch, and the job says so.
