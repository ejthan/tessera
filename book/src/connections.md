# Connections

## Create and edit

Add connection (⌘N / Ctrl+N) opens the form: name, color, environment, nodes, API key, read-only, and under Advanced the timeout, retries and certificates. Test (⌘T / Ctrl+T) tries the server and tells an admin key from a restricted one. Save and connect is ⌘↩ / Ctrl+Enter, and ⌥⌘K / Ctrl+Alt+K shows or hides the key.

On a selected sidebar entry:

| Action | Shortcut |
|---|---|
| Connect | Enter |
| Disconnect | ⇧⌘D / Ctrl+Shift+D |
| Edit | ⌘E / Ctrl+E |
| Delete, also removes the stored key | ⌘⌫ / Ctrl+Backspace |

## Environments

Each connection carries an environment: local, dev, staging, prod or a name of your own. It shows in the title bar, on tabs and in the sidebar. **Prod** turns on read-only and draws a red band and a red frame around the window while one of its tabs is active.

## Read-only

A read-only connection blocks every write request. To change something, Tessera asks you to unlock it by typing the profile name. A "Writes unlocked" banner then stays above the tabs, with Lock again (⇧⌘L / Ctrl+Shift+L). A disconnect locks it again, and an unlock is never saved.

## Clusters

Add node for a cluster adds more hosts to one connection. The Overview shows a card per node with its role and latency. Click a card or press Enter on it to make that node active. Tessera fails over between nodes.

## Overview

The Overview shows the version, collections, documents, memory, CPU, disk and network of a server, with charts of the last 15 minutes and API stats. Space pauses polling, and polling runs every 2, 5, 10 or 30 seconds. ⌘R / Ctrl+R reloads the collections.

## Moving profiles

More actions in the sidebar offers Import from typesense-dashboard, Export profiles, which writes JSON without keys, and Reopen last connection at startup.

A connection opened with [command-line flags](cli.md) shows as "not saved", and Save as profile keeps it.

## Connection warnings

A banner above every tab says when a connection sends `http://` to a remote host or accepts any certificate. It has no close button. Edit connection (⌥⌘X / Ctrl+Alt+X) opens the profile.
