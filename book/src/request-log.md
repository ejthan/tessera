# Request log

⌘J / Ctrl+J opens the log. It lists every request Tessera sends, with time, method, path, node, status, duration and size, and it filters by errors, writes, methods, connections and polling traffic.

The detail view shows headers and bodies with secrets masked.

- **Copy as curl** (⇧⌘C / Ctrl+Shift+C) puts a command on the clipboard that reads the key from `$TYPESENSE_API_KEY`, so export that variable before you paste it into a shell.
- **Show full** (⇧⌘B / Ctrl+Shift+B) opens a long body in chunks, with Previous and Next (⌥⌘[ and ⌥⌘] / Ctrl+Alt+[ and Ctrl+Alt+]) and Copy all.

The log stays in memory and is gone when you quit. It keeps 5,000 of your requests and 500 polling requests. Import batches and document pages keep no body, and the detail says so.

## The last failure

After a failed request the status bar offers Last failed request (⌥⌘R / Ctrl+Alt+R, also in the palette). It opens the log on the newest failure of the active connection, resets a filter that hid it, and selects it. That need not be the error on screen. Polling and other connections stay out.
