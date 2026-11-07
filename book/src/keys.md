# API keys

API keys (⌥⌘6 / Ctrl+Alt+6) lists each key with its id, description, first characters, actions, collections and expiry, and marks one that ended or ends within a week.

## Create

New (⇧⌘N / Ctrl+Shift+N) asks for a description and offers Search only, Import only and Admin as starting points. A date means the end of that day, local time, and an empty one never expires.

The value shows **once**, in a dialog that only Done (⌘↩ / Ctrl+Enter) closes. Copy is ⌥⌘C / Ctrl+Alt+C, and Save to a connection profile… keeps it. It stays out of the log and of history.

## Delete

⌘⌫ / Ctrl+Backspace says that clients and every scoped key made from this key stop working. It needs the description typed on prod. Deleting the key the connection itself uses says that the connection ends.

## Scoped keys

⌘2 / Ctrl+2 shows a panel that makes a scoped key from a parent key and JSON parameters as you type, without the server. It also decodes a pasted key into its prefix, expiry and parameters, and checks the signature against the parent.

Test (⌘↩ / Ctrl+Enter) searches a collection with that key and shows how many documents it sees, also on a read-only connection. Nothing in the panel is stored.
