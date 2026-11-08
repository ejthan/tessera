# Settings

Open the dialog with ⌘, / Ctrl+, the gear in the title bar or the palette. A change applies at once and saves to `settings.toml`. A number out of range says so under its row and changes nothing.

## Appearance

Theme, Font size, Table density and Language. An explicit theme stays through a change of the system appearance. Text held in open inputs keeps its language until they are reopened.

## Defaults

They apply to new tabs, dialogs and connections.

- **Documents per page**
- **Import batch size**
- **Polling interval**
- **Ask for the name above (documents):** the count above which a bulk action asks you to type the name. Prod always asks, at any count.

## Privacy

Check for updates at start is off by default. On, Tessera asks `api.github.com` for the latest release once at each start and on Check now. The request holds the version number and no key, connection or data.

## Storage

The config, data and log folders, each with Copy. Open logs is ⌥⌘L / Ctrl+Alt+L. Clear cache asks first, removes the temp copies that backups and reindex runs left behind, and reloads the collection lists. What a running job uses stays, and history and saved queries are not touched.

## About

The version, the license and the trademark notice.
