# Command-line flags

Tessera can open a temporary connection, or a saved profile, from the command line. On Windows a release build has no console, so `tessera --help` prints nothing there and this page is the reference.

```bash
tessera --host localhost --port 8108 --api-key-stdin <<< xyz
```

## Connection

Each option has an environment variable, except where the table says none. A flag wins over its variable.

| Flag | Variable | Meaning |
|---|---|---|
| `--host <host>` | `TYPESENSE_HOST` | Open a temporary connection to this host. |
| `--port <port>` | `TYPESENSE_PORT` | Port. Default 8108. |
| `--protocol <http\|https>` | `TYPESENSE_PROTOCOL` | Default: `http` for localhost, otherwise `https`. |
| `--path <path>` | `TYPESENSE_PATH` | URL path prefix. |
| `--env <local\|dev\|staging\|prod>` | none | Environment. Default: `local` for localhost, otherwise `dev`. |
| `--read-only` | none | Block write requests. Always on for `--env prod`. |
| `--api-key-stdin` | `TYPESENSE_API_KEY` | Read the API key as one line from stdin. |

A key is never a flag value, because it would show in the process list. A temporary connection shows as "not saved" and can be saved as a profile.

## Other

| Flag | Meaning |
|---|---|
| `--profile <name>` | Open a saved profile. Cannot be combined with connection options. |
| `--help` | Show the usage text. |
