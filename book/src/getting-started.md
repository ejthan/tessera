# Getting started

## Your first connection

The welcome screen offers two ways in.

**You have a server.** Choose Add connection, enter the host, port and API key, and press Test (⌘T / Ctrl+T). Then Save and connect (⌘↩ / Ctrl+Enter).

**You want to try Tessera on your own machine.** Run Typesense locally shows a Docker command. Copy it and run it in a terminal; Tessera shows the command and never runs it. Then choose Connect to it. That opens the connection form filled in for `http://localhost:8108`, with environment local and the key left for you to enter, or your saved `local` profile for editing if there is one. Nothing connects until you do. The command sets the key to `xyz`, which is for this local server only.

If you use typesense-dashboard, Import from typesense-dashboard reads its `config.json` and shows a preview first.

## The window

- **Sidebar:** your connections. A connected entry opens into rows for its Overview, Collections, Search, Console and the resources of the server.
- **Tabs:** each opens beside the others, with the connection's color and environment on it. ⌘W / Ctrl+W closes a tab and leaves the connection open.
- **Title bar:** where you are, as connection, section and collection. The connection opens its overview, and the section of a collection tab opens the list. The Search everything field opens the palette.
- **Status bar:** the active node, its role and latency, your jobs and the request log.
- **Bottom dock:** the request log (⌘J / Ctrl+J) and Jobs (⇧⌘J / Ctrl+Shift+J).

## Find anything

⌘K / Ctrl+K opens the palette. It lists what the focused view can do now, the collections of open connections, saved queries and connections, each with its shortcut. ⌘P / Ctrl+P lists collections only.

An action from the palette runs with the same unlock and confirmation as its control.

## Several windows

New window (⌥⌘N / Ctrl+Alt+N) opens a second window, for example prod beside staging. A connection lives in one window: opening it from another brings its window forward, and closing the window frees the connection. Close window is ⌥⌘W / Ctrl+Alt+W. Closing the last window quits Tessera on every platform.
