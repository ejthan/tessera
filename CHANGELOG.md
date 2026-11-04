# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Connect to a Typesense server by URL and API key and see its version, Raft role and health.
- Welcome screen with the Docker command for a local Typesense server.
- Errors from the server are shown in full inside the connect dialog.
- English and German UI, chosen from the system language.
- CI on macOS, Linux and Windows: lint, tests, integration tests against Typesense 30.0 and 30.2, release builds with a size check and a smoke start.
