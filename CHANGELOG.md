# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Connect to a Typesense server by URL and API key and see its version, Raft role and health.
- Welcome screen with the Docker command for a local Typesense server.
- Errors from the server are shown in full inside the connect dialog.
- English and German UI, chosen from the system language.
- Connection dialog for new and edited profiles: name, color, environment, cluster nodes, read-only, timeout and retries. Test tells an admin key from a restricted one, keys go to the system keychain, and a keychain failure offers "Use for this session only".
- Delete a saved profile (⌘⌫ / Ctrl+Backspace) after a confirmation that removes its stored key too; import profiles from a typesense-dashboard `config.json` with a preview; export profiles as JSON without keys; reopen the last connection at startup; save a connection from flags or environment variables as a profile.
- Connection profiles with environment tags (local, dev, staging, prod), a visible prod band and window frame, and a read-only guard that blocks writes.
- Multi-node connections with failover between nodes.
- Docked window layout: the sidebar lists each connection with its Overview, every connection opens in its own tab (color dot, name, environment badge), and the request log panel opens at the bottom with ⌘J / Ctrl+J. The prod band and frame follow the active tab.
- Request log panel: every request with time, method, path, node, status, duration and size (plus the connection while several are open); filters for errors, writes, methods, connections and polling traffic; a detail view with headers and bodies, secrets masked; Copy as curl (⌘⇧C / Ctrl+Shift+C) with a `$TYPESENSE_API_KEY` placeholder.
- Command-line flags `--host`, `--port`, `--protocol`, `--path`, `--env`, `--read-only`, `--api-key-stdin`, `--profile` and `--help`, plus the `TYPESENSE_HOST`, `TYPESENSE_PORT`, `TYPESENSE_PROTOCOL`, `TYPESENSE_PATH` and `TYPESENSE_API_KEY` variables. A key is never accepted as a flag value.
- `THIRD_PARTY_NOTICES.md` with the `webpki-root-certs` notice (CDLA-Permissive-2.0).
- CI on macOS, Linux and Windows: lint, tests, integration tests against Typesense 30.0 and 30.2, release builds with a size check and a smoke start.

### Changed

- Profiles are stored in `connections.json` without keys; keys are stored in the OS keychain.
- The live tests use `http://localhost:18108` by default instead of port 8108. The dev server still starts on 8108 unless `TYPESENSE_PORT` is set.
