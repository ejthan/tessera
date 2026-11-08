# Relevance resources

Each resource has a row in the sidebar and a tab per connection. Synonyms, curations and stemming start from Typesense 30.

| Tab | Shortcut |
|---|---|
| Aliases | ⌥⌘1 / Ctrl+Alt+1 |
| Synonyms | ⌥⌘2 / Ctrl+Alt+2 |
| Curations | ⌥⌘3 / Ctrl+Alt+3 |
| Stopwords | ⌥⌘4 / Ctrl+Alt+4 |
| Presets | ⌥⌘5 / Ctrl+Alt+5 |

In these tabs, New is ⇧⌘N / Ctrl+Shift+N, Edit ⌘E / Ctrl+E, Delete ⌘⌫ / Ctrl+Backspace and Reload ⌘R / Ctrl+R. Each delete says what breaks, and needs the name typed on prod.

## Aliases

The list shows each alias with its target, documents and what is wrong with it. Repoint (⌘E / Ctrl+E) states what changes.

**Reindex** (⇧⌘R / Ctrl+Shift+R on an alias) opens an assistant. It creates a new collection, copies the documents, compares the counts, switches the alias and can delete the old collection. Until the switch, the old collection and alias keep serving. Unequal counts offer neither the switch nor the delete.

## Synonym and curation sets

A set has tabs for its items (⌘1 / Ctrl+1), JSON (⌘2 / Ctrl+2), Collections (⌘3 / Ctrl+3) and Test (⌘4 / Ctrl+4).

- **Items:** a dialog per item, with a form and JSON. In a curation item, pinned rows move by drag or ⌥↑ and ⌥↓ / Alt+Up and Alt+Down.
- **Collections:** attach and detach a set. A set that collections name but the server lacks stands above the table, with Detach (⇧⌘K / Ctrl+Shift+K) and New with that name (⇧⌘M / Ctrl+Shift+M).
- **Test:** runs one search twice beside each other (⌘↩ / Ctrl+Enter) and says what each side means. It only reads, so it runs on a locked connection. An unattached curation set cannot be tested, because a search cannot name one.

## Stopwords

One word per line, with a locale. Save shows the words the server stored, because it lowercases them, splits at spaces and removes duplicates.

## Presets

Each preset lists as a search, a multi-search or a union. Open (⌘E / Ctrl+E) and New load the Search tab in preset mode, and Save (⌘S / Ctrl+S) writes to the preset.

## Stemming

Stemming (⌥⌘8 / Ctrl+Alt+8) lists the dictionaries beside their words. Import (⇧⌘N / Ctrl+Shift+N) checks a file whole and names up to 100 bad lines before anything is sent. A word that is already in a dictionary keeps its root, so a changed root needs a delete and a new import.
