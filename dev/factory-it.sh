#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Live integration tests inside the factory sandbox, which has no Docker: start the
# typesense-server binary from .factory/Dockerfile on the port the live tests default
# to, then run them. Locally, use dev/docker-compose.yml instead (docs/building.md).
set -euo pipefail

port=18108
if ! curl -sf "http://localhost:$port/health" >/dev/null; then
  mkdir -p /tmp/typesense-data
  typesense-server --data-dir /tmp/typesense-data --api-key=xyz --api-port="$port" \
    --enable-cors=false >/tmp/typesense.log 2>&1 &
fi
# The live tests poll /health themselves until the server is up.
cargo test -p tessera-client --test live --locked -- --ignored
