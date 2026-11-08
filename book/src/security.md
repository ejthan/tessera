# Security

## Keys

API keys live in your system's credential store: the macOS Keychain, the Windows Credential Manager or the Secret Service on Linux. Saved profiles hold no key. A key is never a command-line flag.

## What Tessera writes

- **Log files** mask keys and secret fields, and never hold request bodies. The log folder is private to your user.
- **The request log** stays in memory.
- **History and saved queries** hold no parameter that carries a key.
- **Backups** hold your documents in plain text, in files private to your user, and no provider key. Keep them as you would keep the data.
- **Copied commands and code** read the key from `TYPESENSE_API_KEY`.

## The network

Tessera connects to the nodes you configured and, if you turn on the update check, to `api.github.com`. It sends no telemetry and writes no crash reports.

A banner above every tab warns when a connection sends `http://` to a remote host or accepts any certificate.

## Safeguards

- Prod connections start read-only, and unlocking needs the profile's name.
- An action that is hard to undo says what it does with real numbers, and the riskiest ones need a name typed.
- Destructive buttons are red and never hold the default focus.

## Reporting a problem

`SECURITY.md` in the repository describes how to report a vulnerability.
