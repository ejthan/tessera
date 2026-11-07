# Troubleshooting

## Where the logs are

Settings → Storage shows the log folder, and Open logs (⌥⌘L / Ctrl+Alt+L) opens it from there. By default it is:

| OS | Folder |
|---|---|
| macOS | `~/Library/Application Support/org.tessera.Tessera/logs` |
| Windows | `%LOCALAPPDATA%\tessera\Tessera\data\logs` |
| Linux | `~/.local/share/tessera/logs` |

Each day has its own `tessera.log.YYYY-MM-DD`, and the last 7 days are kept. The default level is `info`. To see more, start Tessera with `RUST_LOG=debug`. Keys stay masked at every level.

For a request that failed, Last failed request (⌥⌘R / Ctrl+Alt+R) is the quicker way: see [Request log](request-log.md).

## "The server answers, but this key may not read its status"

A search-only key gets the same refusal as a wrong key, because every view needs status calls that such a key may not make. So Test stays failed and offers Save anyway. A restricted key keeps working for its own actions, but Tessera cannot browse with it. To browse, use a key with more rights. If you expected this key to work, check it for a typo first.

## Linux: "Could not store the API key"

Tessera stores keys through the Secret Service. A desktop without one, such as a bare window manager or a server session, has nothing to store them in. Install and start a provider, such as GNOME Keyring or KeePassXC with its Secret Service integration.

Until then:

- Choose Use for this session only in the connection form. The key is gone when you quit.
- Or pass the key at start with `--api-key-stdin` or `TYPESENSE_API_KEY`: see [Command-line flags](cli.md).

## Linux: the window does not open

Tessera needs a Wayland or X11 session and a working Vulkan driver. On Ubuntu, `mesa-vulkan-drivers` provides the open drivers, and `vulkaninfo --summary` from `vulkan-tools` shows whether one is found. On a virtual machine or over remote desktop there may be none. Start Tessera from a terminal to read what it reports.

## macOS: the app will not open

Tessera is not notarized. See [Install](install.md) for `xattr -cr`, and for the keychain prompt after an update.

## Windows: `tessera --help` prints nothing

A release build has no console. See [Command-line flags](cli.md).

## Popular queries stay empty

The server must run with its analytics flags, and Tessera cannot detect them. Flush now (⇧⌘U / Ctrl+Shift+U) in the Analytics tab writes what the server holds.

## A file could not be read

A settings file that cannot be read says why at the top of the Settings dialog and is not written over. A keymap error shows above the tabs. A saved-connections file from a newer version is refused and left alone, and the welcome screen says so.
