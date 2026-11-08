# Analytics and AI

Both start from Typesense 30.

## Analytics

Analytics (⌥⌘7 / Ctrl+Alt+7) lists the rules beside a form that follows the rule's type. A rule of an unknown type stays readable as JSON and can be deleted.

The server must run with its analytics flags, which Tessera cannot detect, so the tab always says so.

| Action | Shortcut |
|---|---|
| Flush now | ⇧⌘U / Ctrl+Shift+U |
| Send a test event | ⌘T / Ctrl+T |
| Open the query in search | ⌥⌘G / Ctrl+Alt+G |

## AI

AI (⌥⌘9 / Ctrl+Alt+9) lists conversation models (⌘1 / Ctrl+1) and NL search models (⌘2 / Ctrl+2) beside a form.

The API key field is masked and starts empty on an edit, with a note that the stored key stays. An edit sends only what changed. Errors show in the tab with the typed key removed, because a provider can echo it.

Deleting a conversation model says that the history documents stay.

## Test panels

Under a saved model, pick a collection, a `query_by` and a question.

- **Chat:** a turn (⌘↩ / Ctrl+Enter) calls the provider and stores the turn, so it asks to unlock a read-only connection. New conversation is ⇧⌘A / Ctrl+Shift+A.
- **NL search:** runs by button on any connection, and shows what the model generated, what the server searched with, and the hits.

A run can cost money at the provider, so a search that reaches a model is never retried. A timeout says that the server may still be calling the provider.
