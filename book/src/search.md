# Search

⇧⌘E / Ctrl+Shift+E or Search in the sidebar opens the Search tab of a connection.

## Build a search

Edit it as a form (⌘1 / Ctrl+1) or as JSON (⌘2 / Ctrl+2). ⌘↩ / Ctrl+Enter runs it. ⌥⌘L / Ctrl+Alt+L turns on instant search, which runs 150 ms after the last change, and is off by default on prod. A query that calls a model waits for Run.

## Results

Hits are on ⌘3 / Ctrl+3 and the raw response on ⌘4 / Ctrl+4. Up and Down or a click show a document and its match details beside the list. A checked facet value goes into `filter_by` and runs. ⌥⌘↑ / Ctrl+Alt+Up and ⌥⌘↓ / Ctrl+Alt+Down change the page.

A search error shows in full over the dimmed result.

## Several searches

The tab holds up to 50 searches, sent together.

| Action | Shortcut |
|---|---|
| Add a copy of the active search | ⌘T / Ctrl+T |
| Remove it | ⇧⌘W / Ctrl+Shift+W |
| Next or previous search | ⌥⌘→ ⌥⌘← / Ctrl+Alt+Right, Ctrl+Alt+Left |
| Union, merges them into one list | ⇧⌘M / Ctrl+Shift+M |

## History, saved queries and presets

| Action | Shortcut |
|---|---|
| History of this connection | ⇧⌘H / Ctrl+Shift+H |
| Saved queries | ⌘O / Ctrl+O |
| Save this search | ⌘S / Ctrl+S |
| Presets of the server | ⌥⌘P / Ctrl+Alt+P |
| Leave preset mode | ⇧⌘G / Ctrl+Shift+G |

Enter loads a row and runs nothing. History holds no parameter that carries a key, and a temporary connection keeps no history.

## Export a search

⌥⌘E / Ctrl+Alt+E copies the search as curl, JavaScript, Python, Rust or a URL. ⌥⌘U / Ctrl+Alt+U copies curl directly. The code reads the key from `TYPESENSE_API_KEY`, so no export holds a key.
