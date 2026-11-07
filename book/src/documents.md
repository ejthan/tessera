# Documents

A collection tab opens on its documents. ⌘1 / Ctrl+1 shows the documents and ⌘2 / Ctrl+2 the schema.

## Browse

The table spans all matching documents and loads pages as they scroll into view. Click a header or press ⇧⌘S / Ctrl+Shift+S on a cell to sort on the server. The status line shows the found and total counts and the search time.

| Action | Shortcut |
|---|---|
| Columns popover: hide, show, pin, reset | ⇧⌘O / Ctrl+Shift+O |
| Pin the selected cell's column | ⇧⌘P / Ctrl+Shift+P |
| Narrower or wider by 20 px | ⇧⌘← ⇧⌘→ / Ctrl+Shift+Left, Ctrl+Shift+Right |
| Go to a document by id | ⌘G / Ctrl+G |

Column choices are kept per connection and collection.

## Filter

The bar above the table takes a `filter_by` and completes fields, operators and values of facet fields. ⌘F / Ctrl+F focuses it, Enter applies, and ⇧⌘F / Ctrl+Shift+F clears it. A filter the server refuses shows its message under the bar, and the old rows stay dimmed.

## Inspect and edit

⌘I / Ctrl+I, Enter or a double click opens the JSON editor beside the table, and it follows the selected row. ⌘↩ / Ctrl+Enter opens a review with a diff and the exact request. ⇧⌘M / Ctrl+Shift+M switches between a PATCH and an upsert, and ⇧⌘R / Ctrl+Shift+R reverts. The document is fetched again first, and a change on the server shows as a warning.

New document (⇧⌘N / Ctrl+Shift+N) opens a template built from the schema.

Disconnecting or quitting drops unsaved changes without asking. Closing the inspector or the tab asks first.

## Select and copy

Click a row's box, Shift+click or Shift+Space for a range, Space to tick the selected row, ⌘A / Ctrl+A for the loaded rows, and Esc to clear. A selection holds up to 1,000 rows, and a new filter or sort clears it.

| Copy | Shortcut |
|---|---|
| The selected cell | ⌘C / Ctrl+C |
| The row as JSON | ⌥⌘C / Ctrl+Alt+C |
| Ticked rows as JSON Lines | ⌥⌘J / Ctrl+Alt+J |
| Ticked rows as CSV | ⌥⌘S / Ctrl+Alt+S |
| A curl command | ⌥⌘U / Ctrl+Alt+U |

A copied curl command reads the key from `$TYPESENSE_API_KEY`.

## Delete and update many

⌘⌫ / Ctrl+Backspace deletes the ticked rows or the selected row after a confirmation. ⇧⌘K / Ctrl+Shift+K deletes by query, and ⇧⌘U / Ctrl+Shift+U updates by query. Each starts from the table's filter and shows a live count. A blank filter, `*` and `id:*` are refused.

Above 1,000 documents, or on prod, the confirmation needs the collection's name typed. You can change that number in [Settings](settings.md), except on prod, which always asks.

An update that reaches fewer documents than it counted stays open as a warning, because the server skips values that do not fit.
